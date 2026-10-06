#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(reader) = libmaxminddb_rs::Reader::from_bytes(data) {
        let mut records = 0;
        let _ = reader.visit_records(|_, _| {
            records += 1;
            if records >= 256 {
                return Err(libmaxminddb_rs::Error::ResourceLimit("fuzz scan callback budget"));
            }
            Ok(())
        });
        let _ = reader.lookup_value("1.2.3.4".parse().unwrap());
        let _ = reader.lookup_value("2001:db8::1".parse().unwrap());
        for ip in ["1.2.3.4", "2001:db8::1"] {
            let ip = ip.parse().unwrap();
            let _ = reader.lookup_exists(ip);
            let _ = reader.lookup_value(ip);
        }
    }
});
