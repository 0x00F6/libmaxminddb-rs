#[cfg(feature = "reader")]
fn main() {
    let data = std::fs::read("target/bench-data/GeoIP2-City-Bench.mmdb").unwrap();
    let r = libmaxminddb_rs::Reader::from_bytes(&data).unwrap();
    let md = r.metadata();
    println!(
        "node_count={} record_size={} ip_version={}",
        md.node_count, md.record_size, md.ip_version
    );
    // force prepared tree build by doing a lookup
    use std::net::IpAddr;
    let ip: IpAddr = "2001:db8::1".parse().unwrap();
    let _ = r.lookup_exists(ip);
}

#[cfg(not(feature = "reader"))]
fn main() {}
