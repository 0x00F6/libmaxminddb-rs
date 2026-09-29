//! Error types used by the crate.

use std::io;

/// Error returned by MMDB parsing, lookup, encoding and writing operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The input does not have a valid MMDB database layout.
    #[error("invalid MaxMind DB database: {0}")]
    InvalidDatabase(&'static str),
    /// The metadata section is absent, malformed, or unsupported.
    #[error("invalid metadata: {0}")]
    InvalidMetadata(&'static str),
    /// A search-tree node index is outside the declared tree.
    #[error("invalid search-tree node {0}")]
    InvalidNode(u64),
    /// A pointer or byte offset points outside the valid database region.
    #[error("invalid offset {0}")]
    InvalidOffset(usize),
    /// An MMDB data type tag is invalid or unsupported.
    #[error("unsupported or invalid MMDB data type {0}")]
    InvalidDataType(u8),
    /// The requested or declared IP version is not IPv4 or IPv6.
    #[error("invalid IP version {0}")]
    InvalidIpVersion(u16),
    /// The database ended before the current value could be decoded.
    #[error("unexpected end of input")]
    UnexpectedEof,
    /// A value cannot be represented by the writer.
    #[error("encoding error: {0}")]
    EncodingError(String),
    /// A value cannot be decoded into the requested representation.
    #[error("decoding error: {0}")]
    DecodingError(String),
    /// No network in the database matches the requested address.
    #[error("lookup did not find a matching network")]
    NotFound,
    /// Defensive parser limits rejected excessively deep or large input.
    #[error("resource limit exceeded: {0}")]
    ResourceLimit(&'static str),
    /// Underlying file or mapping I/O failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Serde JSON conversion used by the convenience owned API failed.
    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
    /// A standard-library IP address string could not be parsed.
    #[error(transparent)]
    AddrParse(#[from] std::net::AddrParseError),
    /// An IP network/CIDR string could not be parsed.
    #[error(transparent)]
    IpNet(#[from] ipnet::AddrParseError),
}

/// Crate-local result alias.
pub type Result<T> = std::result::Result<T, Error>;
