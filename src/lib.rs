#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]
//! Read and write MaxMind DB (MMDB) v2 files in Rust.
//!
//! The crate implements the binary format independently. [`Reader`] opens an
//! owned file, a borrowed byte slice, or an explicitly unsafe memory mapping;
//! [`Writer`] builds MMDB files from IP networks and values. Search-tree
//! traversal, data decoding, metadata, and serialization live in separate
//! modules. The public entry points are re-exported at the crate root.
//!
//! Add `libmaxminddb-rs = "0.4.0"` to the `[dependencies]` section of `Cargo.toml`.
//! See the repository README for benchmark methodology and complete examples.
//!
//! # Read a record
//!
//! ```rust
//! # #[cfg(all(feature = "reader", feature = "writer"))]
//! # mod example {
//! use libmaxminddb_rs::{Error, MetadataBuilder, Reader, Value, Writer};
//! use std::net::IpAddr;
//! use std::collections::BTreeMap;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let metadata = MetadataBuilder::new().ip_version(4).build()?;
//! let mut writer = Writer::with_metadata(metadata);
//! writer.insert_value("198.51.100.0/24".parse()?,
//!     Value::Map(BTreeMap::from([("asn".into(), Value::Uint32(64512))])))?;
//! let bytes = writer.finish()?;
//! let reader = Reader::from_bytes(&bytes)?;
//! let ip: IpAddr = "198.51.100.7".parse()?;
//! let value = reader.lookup_value(ip)?;
//! assert_eq!(value.get("asn").is_some(), true);
//! assert!(matches!(reader.lookup_value("203.0.113.7".parse()?), Err(Error::NotFound)));
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! [`Reader::lookup_borrowed`] decodes directly into `#[derive(MmdbDecode)]`
//! types. `&str` and `&[u8]` fields borrow the underlying MMDB bytes; maps and
//! arrays materialized as [`ValueRef`] allocate their container vectors.
//! `lookup_borrowed_map` can project a large record into a small result. A miss
//! returns [`Error::NotFound`] from result-based methods, while
//! [`Reader::lookup_borrowed_opt`] returns `None` for both misses and decode
//! failures.
//!
//! [`Reader::visit_records`] scans stored network ranges without enumerating
//! IP addresses; [`Reader::visit_borrowed_records`] uses the same checked
//! traversal and decodes directly into borrowed user records. Both require
//! only the reader feature and retain this reader as the owner of source bytes.
//!
//! # Cargo features
//!
//! - `reader`: search and decode MMDB files, including mmap support.
//! - `writer`: serialize networks and values, with configurable merge behavior.
//! - `derive`: derive [`MmdbDecode`], [`MmdbEncode`], and [`MmdbRecord`].
//! - `simd`: enable architecture-specific ASCII scanning where available.
//!
//! The reader always builds its cache-aligned fast tree and accelerator tables
//! during open for every valid record size. This adds to open time and memory
//! use, while keeping index construction out of the lookup path.
//!
//! All four features are enabled by default. A reader-only build can use
//! `default-features = false, features = ["reader"]`. Memory-mapped files
//! must not be changed or truncated while borrowed by a reader; opening one
//! therefore requires an explicit `unsafe` call.

extern crate self as libmaxminddb_rs;

mod error;
mod metadata;
mod network;
mod traits;
mod value;

mod decoder;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod editor;
#[cfg(feature = "writer")]
mod encoder;
#[cfg(feature = "reader")]
pub mod reader;
#[cfg(feature = "reader")]
pub mod reloadable;
#[cfg(feature = "writer")]
pub mod writer;

pub use error::{Error, Result};
pub use metadata::{Metadata, MetadataBuilder};
pub use network::IpNetwork;
pub use traits::{DecodeField, EncodeField, IntoMmdbValue, MmdbDecode, MmdbEncode, MmdbRecord};
pub use value::{Value, ValueRef};

#[cfg(all(feature = "reader", feature = "writer"))]
pub use editor::Editor;
#[cfg(feature = "reader")]
pub use reader::Reader;
#[cfg(feature = "reader")]
pub use reloadable::ReloadableReader;
#[cfg(feature = "writer")]
pub use writer::{MergeStrategy, Writer};

#[cfg(feature = "derive")]
pub use libmaxminddb_rs_derive::{MmdbDecode, MmdbEncode, MmdbRecord};

/// Items referenced by code generated with `#[derive(MmdbDecode)]`.
///
/// Not part of the stable API: names and signatures may change in any release.
#[doc(hidden)]
pub mod __private {
    pub use crate::decoder::{Container, RawDecoder};
}
