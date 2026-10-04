//! Copy-on-write editing of existing MaxMind DB files.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use crate::{IntoMmdbValue, IpNetwork, MergeStrategy, Metadata, Reader, Result, Value, Writer};

#[derive(Debug)]
enum Edit {
    Upsert(Value, MergeStrategy),
    Delete,
}

/// Copy-on-write editor for an existing MMDB.
///
/// Opening does not materialize all records. Changes live in an overlay and
/// unchanged source values are borrowed until the replacement database is built.
#[derive(Debug)]
pub struct Editor<'a> {
    reader: Arc<Reader<'a>>,
    edits: Vec<(IpNetwork, Edit)>,
    pending_networks: HashSet<IpNetwork>,
}

impl Editor<'static> {
    /// Opens an MMDB from disk for editing.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::from_reader(Reader::open(path)?))
    }
}

impl<'a> Editor<'a> {
    /// Opens an MMDB directly from borrowed bytes without copying the source.
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self> {
        Ok(Self::from_reader(Reader::from_bytes(bytes)?))
    }

    /// Creates an editor from an already validated reader.
    #[must_use]
    pub fn from_reader(reader: impl Into<Arc<Reader<'a>>>) -> Self {
        Self {
            reader: reader.into(),
            edits: Vec::new(),
            pending_networks: HashSet::new(),
        }
    }

    /// Returns the shared immutable source, including its prepared search tree.
    #[must_use]
    pub fn source_reader(&self) -> &Arc<Reader<'a>> {
        &self.reader
    }

    /// Returns metadata from the source database.
    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        self.reader.metadata()
    }

    /// Inserts or replaces a prefix with a serde-serializable value.
    pub fn insert<T: serde::Serialize + ?Sized>(
        &mut self,
        network: IpNetwork,
        value: &T,
    ) -> Result<()> {
        self.insert_value(network, Value::from_serialize(value)?)
    }

    /// Inserts or replaces a prefix with an already-typed MMDB value.
    pub fn insert_value(&mut self, network: IpNetwork, value: Value) -> Result<()> {
        validate_family(self.reader.metadata().ip_version, network)?;
        self.pending_networks.insert(network);
        self.edits
            .push((network, Edit::Upsert(value, MergeStrategy::Replace)));
        Ok(())
    }

    /// Replaces a prefix. Existence is resolved while rebuilding, avoiding an eager scan.
    pub fn update<T: serde::Serialize + ?Sized>(
        &mut self,
        network: IpNetwork,
        value: &T,
    ) -> Result<()> {
        self.insert(network, value)
    }

    /// Updates a prefix with an owned value or a borrowed `MmdbEncode` record.
    ///
    /// Each update chooses its own strategy. Updates are replayed in call order
    /// during rebuild, using the writer's exact-prefix merge semantics on the
    /// exported source routes. Inherited parent values and more-specific child
    /// values are not merged into the target prefix. A missing prefix is inserted.
    /// `Replace` discards the previous value; `DeepMerge` recursively merges maps,
    /// appends arrays and replaces scalar conflicts. `Append` and `AppendUnique`
    /// follow the same rules as [`MergeStrategy`]. Encoding errors leave the
    /// overlay unchanged. Only the supplied record is encoded at this call;
    /// source records remain borrowed until rebuild.
    ///
    /// ```rust
    /// # #[cfg(feature = "derive")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use libmaxminddb_rs::{Editor, MergeStrategy, MmdbEncode};
    /// #[derive(MmdbEncode)]
    /// struct Patch { score: u32 }
    /// let source = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"),
    ///     "/tests/fixtures/doc.mmdb"));
    /// let mut editor = Editor::from_bytes(source)?;
    /// editor.update_value("198.51.100.7/32".parse()?,
    ///     &Patch { score: 42 }, MergeStrategy::DeepMerge)?;
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "derive"))] fn main() {}
    /// ```
    pub fn update_value(
        &mut self,
        network: IpNetwork,
        value: impl IntoMmdbValue,
        strategy: MergeStrategy,
    ) -> Result<()> {
        validate_family(self.reader.metadata().ip_version, network)?;
        let value = value.into_mmdb_value()?;
        self.pending_networks.insert(network);
        self.edits.push((network, Edit::Upsert(value, strategy)));
        Ok(())
    }

    /// Removes exactly this CIDR prefix. More-specific child prefixes remain.
    pub fn remove(&mut self, network: IpNetwork) -> Result<()> {
        validate_family(self.reader.metadata().ip_version, network)?;
        self.pending_networks.insert(network);
        self.edits.push((network, Edit::Delete));
        Ok(())
    }

    /// Returns the number of distinct prefixes with pending modifications.
    #[must_use]
    pub fn pending_edits(&self) -> usize {
        self.pending_networks.len()
    }

    /// Rebuilds the database and returns replacement MMDB bytes.
    pub fn finish(self) -> Result<Vec<u8>> {
        let mut metadata = self.reader.metadata().clone();
        metadata.node_count = 0;
        let mut writer = Writer::with_metadata_and_capacity(metadata, 1024);

        self.reader.visit_records(|network, borrowed| {
            writer.insert_value(network, borrowed.to_owned_value())
        })?;

        // Apply the overlay after importing the source. Writer::remove creates
        // an explicit no-data boundary, so deleting a child of an inherited
        // parent does not require splitting or cloning the parent record.
        for (network, edit) in self.edits {
            match edit {
                Edit::Upsert(value, strategy) => {
                    writer = writer.merge_strategy(strategy);
                    writer.insert_value(network, value)?;
                }
                Edit::Delete => writer.remove(network)?,
            }
        }
        writer.finish()
    }

    /// Rebuilds the database and writes it to disk.
    pub fn write_to_file(self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, self.finish()?)?;
        Ok(())
    }
}

fn validate_family(ip_version: u16, network: IpNetwork) -> Result<()> {
    match (ip_version, network) {
        (4, IpNetwork::V4(_)) | (6, _) => Ok(()),
        (4, IpNetwork::V6(_)) => Err(crate::Error::InvalidIpVersion(6)),
        (version, _) => Err(crate::Error::InvalidIpVersion(version)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Error, MetadataBuilder};
    use std::collections::BTreeMap;

    fn value(id: u32) -> Value {
        Value::Map(BTreeMap::from([("id".into(), Value::Uint32(id))]))
    }

    fn fixture() -> Vec<u8> {
        let metadata = MetadataBuilder::new().ip_version(4).build().unwrap();
        let mut writer = Writer::with_metadata(metadata);
        writer
            .insert_value("10.0.0.0/24".parse().unwrap(), value(1))
            .unwrap();
        writer
            .insert_value("10.0.1.0/24".parse().unwrap(), value(2))
            .unwrap();
        writer.finish().unwrap()
    }

    #[test]
    fn edits_existing_database_without_mutating_source() {
        let source = fixture();
        let mut editor = Editor::from_bytes(&source).unwrap();
        editor
            .update_value(
                "10.0.0.0/24".parse().unwrap(),
                value(10),
                MergeStrategy::Replace,
            )
            .unwrap();
        editor.remove("10.0.1.0/24".parse().unwrap()).unwrap();
        editor
            .insert_value("10.0.2.0/24".parse().unwrap(), value(3))
            .unwrap();

        let rebuilt = editor.finish().unwrap();
        let reader = Reader::from_bytes(&rebuilt).unwrap();
        assert_eq!(
            reader
                .lookup_value("10.0.0.7".parse().unwrap())
                .unwrap()
                .get("id"),
            Some(&crate::ValueRef::Uint32(10))
        );
        assert!(matches!(
            reader.lookup_value("10.0.1.7".parse().unwrap()),
            Err(Error::NotFound)
        ));
        assert_eq!(
            reader
                .lookup_value("10.0.2.7".parse().unwrap())
                .unwrap()
                .get("id"),
            Some(&crate::ValueRef::Uint32(3))
        );

        let original = Reader::from_bytes(&source).unwrap();
        assert_eq!(
            original
                .lookup_value("10.0.0.7".parse().unwrap())
                .unwrap()
                .get("id"),
            Some(&crate::ValueRef::Uint32(1))
        );
    }

    #[test]
    fn last_overlay_operation_wins() {
        let source = fixture();
        let mut editor = Editor::from_bytes(&source).unwrap();
        let net = "10.0.0.0/24".parse().unwrap();
        editor.remove(net).unwrap();
        editor.insert_value(net, value(42)).unwrap();
        assert_eq!(editor.pending_edits(), 1);
        let rebuilt = editor.finish().unwrap();
        let reader = Reader::from_bytes(&rebuilt).unwrap();
        assert_eq!(
            reader
                .lookup_value("10.0.0.1".parse().unwrap())
                .unwrap()
                .get("id"),
            Some(&crate::ValueRef::Uint32(42))
        );
    }

    #[test]
    fn removal_blocks_parent_inheritance_and_preserves_specific_children() {
        let metadata = MetadataBuilder::new().ip_version(4).build().unwrap();
        let mut writer = Writer::with_metadata(metadata);
        writer
            .insert_value("10.20.0.0/16".parse().unwrap(), value(1))
            .unwrap();
        writer
            .insert_value("10.20.2.0/24".parse().unwrap(), value(2))
            .unwrap();
        let source = writer.finish().unwrap();

        let mut editor = Editor::from_bytes(&source).unwrap();
        editor.remove("10.20.1.0/24".parse().unwrap()).unwrap();
        let rebuilt = editor.finish().unwrap();
        let reader = Reader::from_bytes(&rebuilt).unwrap();

        assert!(matches!(
            reader.lookup_value("10.20.1.7".parse().unwrap()),
            Err(Error::NotFound)
        ));
        assert_eq!(
            reader
                .lookup_value("10.20.3.7".parse().unwrap())
                .unwrap()
                .get("id"),
            Some(&crate::ValueRef::Uint32(1))
        );
        assert_eq!(
            reader
                .lookup_value("10.20.2.7".parse().unwrap())
                .unwrap()
                .get("id"),
            Some(&crate::ValueRef::Uint32(2))
        );
    }

    #[test]
    fn rejects_ipv6_edit_for_ipv4_database() {
        let source = fixture();
        let mut editor = Editor::from_bytes(&source).unwrap();
        assert!(matches!(
            editor.remove("2001:db8::/32".parse().unwrap()),
            Err(Error::InvalidIpVersion(6))
        ));
    }
}
