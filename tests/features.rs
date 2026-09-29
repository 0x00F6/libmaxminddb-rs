use libmaxminddb_rs::{MetadataBuilder, Value};

#[test]
fn metadata_builder_rejects_bad_ip_version() {
    assert!(MetadataBuilder::new().ip_version(5).build().is_err());
}

#[test]
fn value_rejects_json_null() {
    assert!(Value::from_serialize(&serde_json::Value::Null).is_err());
}
