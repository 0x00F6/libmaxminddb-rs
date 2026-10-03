//! Immutable reader snapshots with atomic publication through `arc-swap`.

use std::path::Path;
use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};

use crate::{Reader, Result};

/// RCU-like container for a prepared, immutable MMDB reader.
///
/// Readers keep one guard per query or batch. Publishing never changes the
/// bytes or prepared tree of an existing snapshot. Old generations remain
/// alive until their last guard, snapshot or editor is dropped. Retaining
/// snapshots therefore retains their database memory.
///
/// Borrowed lookup results must not outlive their guard. Use [`Self::snapshot`]
/// for a long-lived or async task. The container does not watch files or persist
/// edits: publication changes the in-memory reader only. Mmap readers retain
/// their original safety contract; never overwrite or truncate mapped files.
///
/// ```
/// use libmaxminddb_rs::{Reader, ReloadableReader};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"),
///     "/tests/fixtures/doc.mmdb"));
/// let database = ReloadableReader::new(Reader::from_bytes(bytes)?);
/// let old = database.snapshot();
/// database.replace(Reader::from_bytes(bytes)?);
/// assert_eq!(old.metadata().ip_version, 4);
/// assert_eq!(database.load().metadata().ip_version, 4);
/// # Ok(())
/// # }
/// ```
pub struct ReloadableReader<'a> {
    current: ArcSwap<Reader<'a>>,
}

impl<'a> ReloadableReader<'a> {
    /// Creates a container without copying bytes or rebuilding a prepared tree.
    #[must_use]
    pub fn new(reader: impl Into<Arc<Reader<'a>>>) -> Self {
        Self {
            current: ArcSwap::from(reader.into()),
        }
    }

    /// Pins a consistent generation for short queries without a global lock.
    #[must_use]
    #[inline]
    pub fn load(&self) -> Guard<Arc<Reader<'a>>> {
        self.current.load()
    }

    /// Returns an owned snapshot suitable for long-lived tasks and editors.
    #[must_use]
    #[inline]
    pub fn snapshot(&self) -> Arc<Reader<'a>> {
        self.current.load_full()
    }

    /// Publishes a constructed reader atomically and returns the old generation.
    /// Concurrent unconditional replacements have last-publication-wins semantics.
    pub fn replace(&self, reader: impl Into<Arc<Reader<'a>>>) -> Arc<Reader<'a>> {
        self.current.swap(reader.into())
    }

    /// Publishes only if `expected` is still the active reader, by Arc identity.
    /// Returns false on conflict; the active reader then remains unchanged.
    /// Keep `expected` alive throughout the operation to prevent pointer reuse.
    pub fn compare_and_replace(
        &self,
        expected: &Arc<Reader<'a>>,
        replacement: impl Into<Arc<Reader<'a>>>,
    ) -> bool {
        let previous = self.current.compare_and_swap(expected, replacement.into());
        Arc::ptr_eq(&previous, expected)
    }

    /// Begins copy-on-write editing from one consistent shared generation.
    #[cfg(feature = "writer")]
    #[must_use]
    pub fn editor(&self) -> crate::Editor<'a> {
        crate::Editor::from_reader(self.snapshot())
    }
}

impl ReloadableReader<'static> {
    /// Opens an owned file and prepares its index before exposing the container.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::new(Reader::open(path)?))
    }

    /// Prepares owned replacement bytes before atomically publishing them.
    /// An opening error leaves the active reader unchanged. The Vec is moved.
    pub fn replace_from_vec(&self, bytes: Vec<u8>) -> Result<Arc<Reader<'static>>> {
        Ok(self.replace(Reader::from_vec(bytes)?))
    }

    /// Loads an owned replacement file before publishing it atomically.
    /// An I/O or opening error leaves the active reader unchanged.
    pub fn reload(&self, path: impl AsRef<Path>) -> Result<Arc<Reader<'static>>> {
        Ok(self.replace(Reader::open(path)?))
    }

    /// Rebuilds an editor and prepares the replacement outside the publication.
    /// Returns false if its source generation has been superseded. On conflict
    /// the editor is consumed; create a fresh editor and reapply desired edits.
    /// Construction errors leave the active generation unchanged. This method
    /// does not persist a file. Rebuild still materializes unchanged records;
    /// sharing the source and publication themselves never copy its bytes.
    #[cfg(feature = "writer")]
    pub fn commit(&self, editor: crate::Editor<'static>) -> Result<bool> {
        let expected = Arc::clone(editor.source_reader());
        let replacement = Reader::from_vec(editor.finish()?)?;
        // Identity comparison also rejects editors from unrelated databases.
        let previous = self
            .current
            .compare_and_swap(&expected, Arc::new(replacement));
        Ok(Arc::ptr_eq(&previous, &expected))
    }
}
