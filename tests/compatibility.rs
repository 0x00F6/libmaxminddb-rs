#![cfg(feature = "reader")]

use std::net::IpAddr;

use libmaxminddb_rs::Reader;

/// Optional compatibility test against an externally supplied official MMDB.
/// Set `MMDB_COMPAT_DB` and optionally `MMDB_COMPAT_IP` to enable it.
#[test]
fn reads_external_mmdb_when_configured() {
    let Ok(path) = std::env::var("MMDB_COMPAT_DB") else {
        eprintln!("MMDB_COMPAT_DB not set; external compatibility test skipped");
        return;
    };
    let ip: IpAddr = std::env::var("MMDB_COMPAT_IP")
        .unwrap_or_else(|_| "81.2.69.160".into())
        .parse()
        .unwrap();
    let reader = Reader::open(path).unwrap();
    let value = reader.lookup_value(ip).unwrap();
    assert!(matches!(value, libmaxminddb_rs::ValueRef::Map(_)));
}
