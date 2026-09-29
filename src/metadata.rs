//! MaxMind DB metadata model and builder.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "writer")]
use crate::Value;
#[cfg(feature = "reader")]
use crate::ValueRef;
use crate::{Error, Result};

/// Public MMDB metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    /// Number of nodes in the binary search tree.
    pub node_count: u64,
    /// Width, in bits, of each tree record (normally 24, 28, or 32).
    pub record_size: u16,
    /// Address-family version stored by the database: 4 or 6.
    pub ip_version: u16,
    /// Application-defined database type identifier.
    pub database_type: String,
    /// Languages advertised by the database.
    pub languages: Vec<String>,
    /// MMDB binary-format major version.
    pub binary_format_major_version: u16,
    /// MMDB binary-format minor version.
    pub binary_format_minor_version: u16,
    /// Database build timestamp as seconds since the Unix epoch.
    pub build_epoch: u64,
    /// Human-readable descriptions keyed by language code.
    pub description: BTreeMap<String, String>,
}

impl Metadata {
    #[cfg(feature = "reader")]
    pub(crate) fn from_value(value: &ValueRef<'_>) -> Result<Self> {
        let map = match value {
            ValueRef::Map(v) => v,
            _ => return Err(Error::InvalidMetadata("metadata root is not a map")),
        };
        let get = |key: &str| map.iter().find_map(|(k, v)| (*k == key).then_some(v));
        Ok(Self {
            node_count: get_u64(get("node_count"))
                .ok_or(Error::InvalidMetadata("missing node_count"))?,
            record_size: get_u16(get("record_size"))
                .ok_or(Error::InvalidMetadata("missing record_size"))?,
            ip_version: get_u16(get("ip_version"))
                .ok_or(Error::InvalidMetadata("missing ip_version"))?,
            database_type: get_string(get("database_type"))
                .ok_or(Error::InvalidMetadata("missing database_type"))?,
            languages: get_strings(get("languages")).unwrap_or_default(),
            binary_format_major_version: get_u16(get("binary_format_major_version")).ok_or(
                Error::InvalidMetadata("missing binary_format_major_version"),
            )?,
            binary_format_minor_version: get_u16(get("binary_format_minor_version")).ok_or(
                Error::InvalidMetadata("missing binary_format_minor_version"),
            )?,
            build_epoch: get_u64(get("build_epoch"))
                .ok_or(Error::InvalidMetadata("missing build_epoch"))?,
            description: get_descriptions(get("description")).unwrap_or_default(),
        })
    }

    #[cfg(feature = "writer")]
    pub(crate) fn to_value(&self) -> Value {
        let mut m = BTreeMap::new();
        m.insert("node_count".into(), Value::Uint32(self.node_count as u32));
        m.insert("record_size".into(), Value::Uint16(self.record_size));
        m.insert("ip_version".into(), Value::Uint16(self.ip_version));
        m.insert(
            "database_type".into(),
            Value::Utf8(self.database_type.clone()),
        );
        m.insert(
            "languages".into(),
            Value::Array(self.languages.iter().cloned().map(Value::Utf8).collect()),
        );
        m.insert(
            "binary_format_major_version".into(),
            Value::Uint16(self.binary_format_major_version),
        );
        m.insert(
            "binary_format_minor_version".into(),
            Value::Uint16(self.binary_format_minor_version),
        );
        m.insert("build_epoch".into(), Value::Uint64(self.build_epoch));
        m.insert(
            "description".into(),
            Value::Map(
                self.description
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::Utf8(v.clone())))
                    .collect(),
            ),
        );
        Value::Map(m)
    }
}

/// Builder for writer metadata.
#[derive(Debug, Clone)]
pub struct MetadataBuilder {
    metadata: Metadata,
}

impl Default for MetadataBuilder {
    fn default() -> Self {
        let build_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Self {
            metadata: Metadata {
                node_count: 0,
                record_size: 28,
                ip_version: 6,
                database_type: "libmaxminddb-rs".into(),
                languages: vec!["en".into()],
                binary_format_major_version: 2,
                binary_format_minor_version: 0,
                build_epoch,
                description: BTreeMap::new(),
            },
        }
    }
}

impl MetadataBuilder {
    /// Creates a builder with MMDB v2 defaults.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::MetadataBuilder;
    /// let metadata = MetadataBuilder::new().build()?;
    /// assert_eq!(metadata.ip_version, 6);
    /// # Ok::<(), libmaxminddb_rs::Error>(())
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the database type identifier.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::MetadataBuilder;
    /// let metadata = MetadataBuilder::new().database_type("Example-City").build()?;
    /// assert_eq!(metadata.database_type, "Example-City");
    /// # Ok::<(), libmaxminddb_rs::Error>(())
    /// ```
    #[must_use]
    pub fn database_type(mut self, value: impl Into<String>) -> Self {
        self.metadata.database_type = value.into();
        self
    }

    /// Sets the database IP version (`4` or `6`).
    /// [`Self::build`] rejects any other value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::MetadataBuilder;
    /// let metadata = MetadataBuilder::new().ip_version(4).build()?;
    /// assert_eq!(metadata.ip_version, 4);
    /// # Ok::<(), libmaxminddb_rs::Error>(())
    /// ```
    #[must_use]
    pub fn ip_version(mut self, value: u16) -> Self {
        self.metadata.ip_version = value;
        self
    }

    /// Adds one advertised language, deduplicating the list.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::MetadataBuilder;
    /// let metadata = MetadataBuilder::new().language("fr").build()?;
    /// assert!(metadata.languages.contains(&"fr".to_string()));
    /// # Ok::<(), libmaxminddb_rs::Error>(())
    /// ```
    #[must_use]
    pub fn language(mut self, value: impl Into<String>) -> Self {
        self.metadata.languages.push(value.into());
        self.metadata.languages.sort();
        self.metadata.languages.dedup();
        self
    }

    /// Replaces the complete advertised-language list.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::MetadataBuilder;
    /// let metadata = MetadataBuilder::new().languages(["fr".to_string()]).build()?;
    /// assert_eq!(metadata.languages, vec!["fr".to_string()]);
    /// # Ok::<(), libmaxminddb_rs::Error>(())
    /// ```
    #[must_use]
    pub fn languages(mut self, values: impl IntoIterator<Item = String>) -> Self {
        self.metadata.languages = values.into_iter().collect();
        self
    }

    /// Adds or replaces a localized database description.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::MetadataBuilder;
    /// let metadata = MetadataBuilder::new().description("en", "Example database").build()?;
    /// assert_eq!(metadata.description["en"], "Example database");
    /// # Ok::<(), libmaxminddb_rs::Error>(())
    /// ```
    #[must_use]
    pub fn description(mut self, language: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata
            .description
            .insert(language.into(), value.into());
        self
    }

    /// Overrides the build epoch in seconds since Unix epoch.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::MetadataBuilder;
    /// let metadata = MetadataBuilder::new().build_epoch(1_700_000_000).build()?;
    /// assert_eq!(metadata.build_epoch, 1_700_000_000);
    /// # Ok::<(), libmaxminddb_rs::Error>(())
    /// ```
    #[must_use]
    pub fn build_epoch(mut self, value: u64) -> Self {
        self.metadata.build_epoch = value;
        self
    }

    /// Validates the configured fields and produces metadata.
    /// Returns [`Error::InvalidIpVersion`] for unsupported IP versions or
    /// [`Error::InvalidMetadata`] for an empty database type.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::MetadataBuilder;
    /// let metadata = MetadataBuilder::new().ip_version(4).build()?;
    /// assert_eq!(metadata.ip_version, 4);
    /// # Ok::<(), libmaxminddb_rs::Error>(())
    /// ```
    pub fn build(self) -> Result<Metadata> {
        if !matches!(self.metadata.ip_version, 4 | 6) {
            return Err(Error::InvalidIpVersion(self.metadata.ip_version));
        }
        if self.metadata.database_type.is_empty() {
            return Err(Error::InvalidMetadata("database_type must not be empty"));
        }
        Ok(self.metadata)
    }
}

#[cfg(feature = "reader")]
fn get_u16(value: Option<&ValueRef<'_>>) -> Option<u16> {
    match value? {
        ValueRef::Uint16(v) => Some(*v),
        ValueRef::Uint32(v) => u16::try_from(*v).ok(),
        ValueRef::Uint64(v) => u16::try_from(*v).ok(),
        _ => None,
    }
}

#[cfg(feature = "reader")]
fn get_u64(value: Option<&ValueRef<'_>>) -> Option<u64> {
    match value? {
        ValueRef::Uint16(v) => Some(u64::from(*v)),
        ValueRef::Uint32(v) => Some(u64::from(*v)),
        ValueRef::Uint64(v) => Some(*v),
        _ => None,
    }
}

#[cfg(feature = "reader")]
fn get_string(value: Option<&ValueRef<'_>>) -> Option<String> {
    match value? {
        ValueRef::Utf8(v) => Some((*v).to_owned()),
        _ => None,
    }
}

#[cfg(feature = "reader")]
fn get_strings(value: Option<&ValueRef<'_>>) -> Option<Vec<String>> {
    match value? {
        ValueRef::Array(v) => v
            .iter()
            .map(|v| match v {
                ValueRef::Utf8(s) => Some((*s).to_owned()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

#[cfg(feature = "reader")]
fn get_descriptions(value: Option<&ValueRef<'_>>) -> Option<BTreeMap<String, String>> {
    match value? {
        ValueRef::Map(v) => v
            .iter()
            .map(|(k, v)| match v {
                ValueRef::Utf8(s) => Some(((*k).to_owned(), (*s).to_owned())),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

#[cfg(all(test, feature = "reader", feature = "writer"))]
mod tests {
    use super::*;

    #[test]
    fn builder_deduplicates_languages_and_serializes_descriptions() {
        let metadata = MetadataBuilder::new()
            .database_type("Coverage-City")
            .ip_version(4)
            .languages(["fr".to_owned()])
            .language("en")
            .language("en")
            .description("en", "first")
            .description("en", "updated")
            .build_epoch(1_700_000_000)
            .build()
            .unwrap();
        assert_eq!(metadata.languages, ["en", "fr"]);
        assert_eq!(metadata.description["en"], "updated");
        assert_eq!(metadata.build_epoch, 1_700_000_000);
        let Value::Map(encoded) = metadata.to_value() else {
            panic!("metadata must encode as a map");
        };
        assert_eq!(
            encoded["database_type"],
            Value::Utf8("Coverage-City".into())
        );
        assert_eq!(
            encoded["description"],
            Value::Map(BTreeMap::from([(
                "en".into(),
                Value::Utf8("updated".into())
            )]))
        );
        assert!(matches!(
            MetadataBuilder::new().ip_version(5).build(),
            Err(Error::InvalidIpVersion(5))
        ));
        assert!(matches!(
            MetadataBuilder::new().database_type("").build(),
            Err(Error::InvalidMetadata(_))
        ));
    }

    #[test]
    fn metadata_decoder_validates_required_and_optional_fields() {
        let fields = vec![
            ("node_count", ValueRef::Uint32(3)),
            ("record_size", ValueRef::Uint16(28)),
            ("ip_version", ValueRef::Uint64(6)),
            ("database_type", ValueRef::Utf8("Coverage-City")),
            ("languages", ValueRef::Array(vec![ValueRef::Utf8("en")])),
            ("binary_format_major_version", ValueRef::Uint16(2)),
            ("binary_format_minor_version", ValueRef::Uint32(0)),
            ("build_epoch", ValueRef::Uint64(1)),
            (
                "description",
                ValueRef::Map(vec![("en", ValueRef::Utf8("example"))]),
            ),
        ];
        let decoded = Metadata::from_value(&ValueRef::Map(fields.clone())).unwrap();
        assert_eq!(decoded.node_count, 3);
        assert_eq!(decoded.description["en"], "example");
        assert_eq!(decoded.languages, ["en"]);
        assert!(Metadata::from_value(&ValueRef::Bool(true)).is_err());

        for required in [
            "node_count",
            "record_size",
            "ip_version",
            "database_type",
            "binary_format_major_version",
            "binary_format_minor_version",
            "build_epoch",
        ] {
            let without = fields
                .iter()
                .filter(|(key, _)| *key != required)
                .cloned()
                .collect();
            assert!(
                Metadata::from_value(&ValueRef::Map(without)).is_err(),
                "{required}"
            );
        }

        let optional_invalid = fields
            .into_iter()
            .map(|(key, value)| match key {
                "languages" => (key, ValueRef::Array(vec![ValueRef::Bool(true)])),
                "description" => (key, ValueRef::Map(vec![("en", ValueRef::Bool(true))])),
                _ => (key, value),
            })
            .collect();
        let decoded = Metadata::from_value(&ValueRef::Map(optional_invalid)).unwrap();
        assert!(decoded.languages.is_empty());
        assert!(decoded.description.is_empty());
    }
}
