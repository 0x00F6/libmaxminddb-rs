#![cfg(all(feature = "reader", feature = "writer"))]

use std::collections::BTreeMap;

use libmaxminddb_rs::{
    Editor, Error, MergeStrategy, MetadataBuilder, MmdbEncode, Reader, Result, Value,
};

fn map(entries: &[(&str, Value)]) -> Value {
    Value::Map(
        entries
            .iter()
            .map(|(key, value)| ((*key).into(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn fixture(value: Value) -> Vec<u8> {
    let mut writer = libmaxminddb_rs::Writer::with_metadata(
        MetadataBuilder::new().ip_version(4).build().unwrap(),
    );
    writer
        .insert_value("10.0.0.0/24".parse().unwrap(), value)
        .unwrap();
    writer.finish().unwrap()
}

fn result(editor: Editor<'_>) -> Value {
    let bytes = editor.finish().unwrap();
    Reader::from_bytes(&bytes)
        .unwrap()
        .lookup_value("10.0.0.1".parse().unwrap())
        .unwrap()
        .to_owned_value()
}

#[test]
fn merges_source_nested_maps_arrays_and_scalar_conflicts() {
    let source = fixture(map(&[
        ("country", Value::Utf8("FR".into())),
        (
            "risk",
            map(&[("score", Value::Uint32(1)), ("vpn", Value::Bool(true))]),
        ),
        ("tags", Value::Array(vec![Value::Uint32(1)])),
    ]));
    let mut editor = Editor::from_bytes(&source).unwrap();
    editor
        .update_value(
            "10.0.0.0/24".parse().unwrap(),
            map(&[
                ("risk", map(&[("score", Value::Uint32(42))])),
                ("tags", Value::Array(vec![Value::Uint32(2)])),
            ]),
            MergeStrategy::DeepMerge,
        )
        .unwrap();
    assert_eq!(
        result(editor),
        map(&[
            ("country", Value::Utf8("FR".into())),
            (
                "risk",
                map(&[("score", Value::Uint32(42)), ("vpn", Value::Bool(true))])
            ),
            (
                "tags",
                Value::Array(vec![Value::Uint32(1), Value::Uint32(2)])
            ),
        ])
    );
}

#[test]
fn all_strategies_match_writer_for_the_same_prefix() {
    for strategy in [
        MergeStrategy::Replace,
        MergeStrategy::Append,
        MergeStrategy::AppendUnique,
        MergeStrategy::DeepMerge,
    ] {
        let old = Value::Array(vec![Value::Uint32(1), Value::Uint32(2)]);
        let new = Value::Array(vec![Value::Uint32(2), Value::Uint32(3)]);
        let source = fixture(old.clone());
        let network = "10.0.0.0/24".parse().unwrap();
        let mut editor = Editor::from_bytes(&source).unwrap();
        editor.update_value(network, new.clone(), strategy).unwrap();
        let mut writer = libmaxminddb_rs::Writer::with_metadata(
            MetadataBuilder::new().ip_version(4).build().unwrap(),
        );
        writer.insert_value(network, old).unwrap();
        writer = writer.merge_strategy(strategy);
        writer.insert_value(network, new).unwrap();
        let expected_bytes = writer.finish().unwrap();
        let expected = Reader::from_bytes(&expected_bytes)
            .unwrap()
            .lookup_value("10.0.0.1".parse().unwrap())
            .unwrap()
            .to_owned_value();
        assert_eq!(result(editor), expected);
    }
}

#[test]
fn successive_updates_keep_their_own_strategy_and_call_order() {
    let source = fixture(map(&[("a", Value::Uint32(1))]));
    let mut editor = Editor::from_bytes(&source).unwrap();
    let network = "10.0.0.0/24".parse().unwrap();
    for (key, strategy) in [
        ("b", MergeStrategy::DeepMerge),
        ("c", MergeStrategy::Replace),
        ("d", MergeStrategy::DeepMerge),
    ] {
        editor
            .update_value(network, map(&[(key, Value::Bool(true))]), strategy)
            .unwrap();
    }
    assert_eq!(editor.pending_edits(), 1);
    assert_eq!(
        result(editor),
        map(&[("c", Value::Bool(true)), ("d", Value::Bool(true))])
    );
}

#[test]
fn remove_then_merge_does_not_resurrect_source_fields() {
    let source = fixture(map(&[("old", Value::Bool(true))]));
    let mut editor = Editor::from_bytes(&source).unwrap();
    let network = "10.0.0.0/24".parse().unwrap();
    editor.remove(network).unwrap();
    editor
        .update_value(
            network,
            map(&[("new", Value::Bool(true))]),
            MergeStrategy::DeepMerge,
        )
        .unwrap();
    assert_eq!(result(editor), map(&[("new", Value::Bool(true))]));
}

#[test]
fn encoding_or_family_errors_leave_overlay_unchanged() {
    struct Failing;
    impl MmdbEncode for Failing {
        fn encode(&self) -> Result<Value> {
            Err(Error::EncodingError("intentional failure".into()))
        }
    }
    let source = fixture(Value::Uint32(1));
    let mut editor = Editor::from_bytes(&source).unwrap();
    assert!(matches!(
        editor.update_value(
            "10.0.0.0/24".parse().unwrap(),
            &Failing,
            MergeStrategy::DeepMerge
        ),
        Err(Error::EncodingError(_))
    ));
    assert!(matches!(
        editor.update_value(
            "2001:db8::/32".parse().unwrap(),
            &Failing,
            MergeStrategy::DeepMerge
        ),
        Err(Error::InvalidIpVersion(6))
    ));
    assert_eq!(editor.pending_edits(), 0);
    assert_eq!(result(editor), Value::Uint32(1));
}

#[test]
fn owned_value_input_moves_its_string_buffer() {
    use libmaxminddb_rs::IntoMmdbValue;
    let text = "owned without cloning".to_owned();
    let address = text.as_ptr();
    let Value::Utf8(moved) = Value::Utf8(text).into_mmdb_value().unwrap() else {
        panic!("expected a string");
    };
    assert_eq!(moved.as_ptr(), address);
}

#[test]
fn borrowed_trait_object_and_missing_prefix_are_supported() {
    struct Patch;
    impl MmdbEncode for Patch {
        fn encode(&self) -> Result<Value> {
            Ok(Value::Uint32(42))
        }
    }
    let patch: &dyn MmdbEncode = &Patch;
    let source = fixture(Value::Uint32(1));
    let mut editor = Editor::from_bytes(&source).unwrap();
    editor
        .update_value(
            "10.0.1.0/24".parse().unwrap(),
            patch,
            MergeStrategy::DeepMerge,
        )
        .unwrap();
    let bytes = editor.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    assert_eq!(
        reader
            .lookup_value("10.0.1.1".parse().unwrap())
            .unwrap()
            .to_owned_value(),
        Value::Uint32(42)
    );
    assert_eq!(
        reader
            .lookup_value("10.0.0.1".parse().unwrap())
            .unwrap()
            .to_owned_value(),
        Value::Uint32(1)
    );
}

#[cfg(feature = "derive")]
#[test]
fn derived_custom_struct_without_serde_supports_deep_merge() {
    #[derive(MmdbEncode)]
    struct RiskPatch<'a> {
        score: u32,
        label: &'a str,
    }
    #[derive(MmdbEncode)]
    struct Patch<'a> {
        risk: RiskPatch<'a>,
    }
    let source = fixture(map(&[
        ("country", Value::Utf8("FR".into())),
        ("risk", map(&[("vpn", Value::Bool(true))])),
    ]));
    let mut editor = Editor::from_bytes(&source).unwrap();
    let patch = Patch {
        risk: RiskPatch {
            score: 42,
            label: "custom",
        },
    };
    editor
        .update_value(
            "10.0.0.0/24".parse().unwrap(),
            &patch,
            MergeStrategy::DeepMerge,
        )
        .unwrap();
    assert_eq!(
        result(editor),
        map(&[
            ("country", Value::Utf8("FR".into())),
            (
                "risk",
                map(&[
                    ("vpn", Value::Bool(true)),
                    ("score", Value::Uint32(42)),
                    ("label", Value::Utf8("custom".into())),
                ])
            ),
        ])
    );
}
