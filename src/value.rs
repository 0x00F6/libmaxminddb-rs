//! Owned and borrowed MMDB values.

use std::collections::BTreeMap;

use serde::ser::{self, Impossible, Serialize};

use crate::{Error, Result};

/// Borrowed representation of a decoded MMDB value.
///
/// String and byte payloads point directly into the source MMDB buffer. Arrays and maps
/// allocate only their small container vectors; their scalar payloads remain borrowed.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueRef<'a> {
    /// Borrowed UTF-8 string.
    Utf8(&'a str),
    /// Borrowed byte string.
    Bytes(&'a [u8]),
    /// IEEE-754 64-bit floating-point value.
    Double(f64),
    /// IEEE-754 32-bit floating-point value.
    Float(f32),
    /// Unsigned 16-bit integer.
    Uint16(u16),
    /// Unsigned 32-bit integer.
    Uint32(u32),
    /// Signed 32-bit integer.
    Int32(i32),
    /// Unsigned 64-bit integer.
    Uint64(u64),
    /// Unsigned 128-bit integer.
    Uint128(u128),
    /// Boolean value.
    Bool(bool),
    /// Ordered MMDB array whose elements may borrow the source buffer.
    Array(Vec<ValueRef<'a>>),
    /// MMDB map represented as borrowed keys and borrowed/inline values.
    Map(Vec<(&'a str, ValueRef<'a>)>),
}

impl<'a> ValueRef<'a> {
    /// Gets a map field without allocating. Returns `None` for missing keys or
    /// non-map values.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::ValueRef;
    /// let value = ValueRef::Map(vec![("asn", ValueRef::Uint32(64512))]);
    /// assert_eq!(value.get("asn"), Some(&ValueRef::Uint32(64512)));
    /// assert_eq!(value.get("missing"), None);
    /// ```
    #[must_use]
    #[inline]
    pub fn get(&self, key: &str) -> Option<&ValueRef<'a>> {
        match self {
            Self::Map(entries) => entries.iter().find_map(|(k, v)| (*k == key).then_some(v)),
            _ => None,
        }
    }

    /// Converts to an owned value. This copies strings, bytes, and container
    /// contents so the result can outlive the MMDB buffer.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{Value, ValueRef};
    /// let borrowed = ValueRef::Utf8("example");
    /// assert_eq!(borrowed.to_owned_value(), Value::Utf8("example".into()));
    /// ```
    #[must_use]
    pub fn to_owned_value(&self) -> Value {
        match self {
            Self::Utf8(v) => Value::Utf8((*v).to_owned()),
            Self::Bytes(v) => Value::Bytes((*v).to_vec()),
            Self::Double(v) => Value::Double(*v),
            Self::Float(v) => Value::Float(*v),
            Self::Uint16(v) => Value::Uint16(*v),
            Self::Uint32(v) => Value::Uint32(*v),
            Self::Int32(v) => Value::Int32(*v),
            Self::Uint64(v) => Value::Uint64(*v),
            Self::Uint128(v) => Value::Uint128(*v),
            Self::Bool(v) => Value::Bool(*v),
            Self::Array(v) => Value::Array(v.iter().map(Self::to_owned_value).collect()),
            Self::Map(v) => Value::Map(
                v.iter()
                    .map(|(k, v)| ((*k).to_owned(), v.to_owned_value()))
                    .collect(),
            ),
        }
    }

    /// Converts this value to JSON for serde-based owned deserialization.
    /// The conversion allocates; 128-bit integers become strings.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::ValueRef;
    /// assert_eq!(ValueRef::Uint32(64512).to_json(), serde_json::json!(64512));
    /// ```
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Utf8(v) => serde_json::Value::String((*v).to_owned()),
            Self::Bytes(v) => {
                serde_json::Value::Array(v.iter().map(|b| serde_json::Value::from(*b)).collect())
            }
            Self::Double(v) => serde_json::json!(v),
            Self::Float(v) => serde_json::json!(v),
            Self::Uint16(v) => serde_json::json!(v),
            Self::Uint32(v) => serde_json::json!(v),
            Self::Int32(v) => serde_json::json!(v),
            Self::Uint64(v) => serde_json::json!(v),
            Self::Uint128(v) => serde_json::Value::String(v.to_string()),
            Self::Bool(v) => serde_json::json!(v),
            Self::Array(values) => {
                serde_json::Value::Array(values.iter().map(Self::to_json).collect())
            }
            Self::Map(entries) => serde_json::Value::Object(
                entries
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), v.to_json()))
                    .collect(),
            ),
        }
    }
}

/// Owned representation used by the writer and deep-merge engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Owned UTF-8 string.
    Utf8(String),
    /// Owned byte string.
    Bytes(Vec<u8>),
    /// IEEE-754 64-bit floating-point value.
    Double(f64),
    /// IEEE-754 32-bit floating-point value.
    Float(f32),
    /// Unsigned 16-bit integer.
    Uint16(u16),
    /// Unsigned 32-bit integer.
    Uint32(u32),
    /// Signed 32-bit integer.
    Int32(i32),
    /// Unsigned 64-bit integer.
    Uint64(u64),
    /// Unsigned 128-bit integer.
    Uint128(u128),
    /// Boolean value.
    Bool(bool),
    /// Owned MMDB array.
    Array(Vec<Value>),
    /// Owned MMDB map with deterministic key ordering.
    Map(BTreeMap<String, Value>),
}

impl Value {
    /// Converts any serde-serializable value into the writer's generic value model.
    ///
    /// Serialization walks the `serde` data model directly instead of building an
    /// intermediate `serde_json::Value`, which roughly halves the per-record cost of
    /// [`Writer::insert`](crate::Writer::insert). Bools, strings, byte strings,
    /// sequences and maps keep their shape; integral numbers become `Uint64` or
    /// `Int32`, floating point becomes `Double`, `None`/unit are rejected because
    /// MMDB has no null type, and enum variants become maps.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::Value;
    /// let value = Value::from_serialize(&serde_json::json!({"asn": 64512}))?;
    /// assert!(matches!(value, Value::Map(_)));
    /// # Ok::<(), libmaxminddb_rs::Error>(())
    /// ```
    pub fn from_serialize<T: serde::Serialize + ?Sized>(value: &T) -> Result<Self> {
        value.serialize(ValueSerializer)
    }
}

fn null_error() -> Error {
    Error::EncodingError("MMDB has no null data type".into())
}

/// Serializer producing [`Value`] without an intermediate JSON tree.
struct ValueSerializer;

/// Accumulates sequence-like values (`seq`, tuple, tuple struct).
#[derive(Default)]
struct SeqSerializer {
    values: Vec<Value>,
}

/// Accumulates object-like values (map, struct).
#[derive(Default)]
struct MapSerializer {
    entries: BTreeMap<String, Value>,
    pending_key: Option<String>,
}

/// Accumulates the fields of a tuple enum variant.
struct TupleVariantSerializer {
    name: &'static str,
    values: Vec<Value>,
}

/// Accumulates the fields of a struct enum variant.
struct StructVariantSerializer {
    name: &'static str,
    entries: BTreeMap<String, Value>,
}

/// Accepts only string-like map keys, matching `serde_json`'s behavior.
struct KeySerializer;

impl serde::ser::Error for Error {
    fn custom<T: std::fmt::Display>(message: T) -> Self {
        Error::EncodingError(message.to_string())
    }
}

impl ser::Serializer for ValueSerializer {
    type Ok = Value;
    type Error = Error;
    type SerializeSeq = SeqSerializer;
    type SerializeTuple = SeqSerializer;
    type SerializeTupleStruct = SeqSerializer;
    type SerializeTupleVariant = TupleVariantSerializer;
    type SerializeMap = MapSerializer;
    type SerializeStruct = MapSerializer;
    type SerializeStructVariant = StructVariantSerializer;

    fn serialize_bool(self, value: bool) -> Result<Self::Ok> {
        Ok(Value::Bool(value))
    }

    fn serialize_i8(self, value: i8) -> Result<Self::Ok> {
        Ok(Value::Int32(i32::from(value)))
    }

    fn serialize_i16(self, value: i16) -> Result<Self::Ok> {
        Ok(Value::Int32(i32::from(value)))
    }

    fn serialize_i32(self, value: i32) -> Result<Self::Ok> {
        Ok(Value::Int32(value))
    }

    fn serialize_i64(self, value: i64) -> Result<Self::Ok> {
        i32::try_from(value)
            .map(Value::Int32)
            .map_err(|_| Error::EncodingError("signed JSON integer does not fit MMDB int32".into()))
    }

    fn serialize_i128(self, value: i128) -> Result<Self::Ok> {
        if let Ok(unsigned) = u64::try_from(value) {
            return Ok(Value::Uint64(unsigned));
        }
        i32::try_from(value)
            .map(Value::Int32)
            .map_err(|_| Error::EncodingError("signed JSON integer does not fit MMDB int32".into()))
    }

    fn serialize_u8(self, value: u8) -> Result<Self::Ok> {
        Ok(Value::Uint64(u64::from(value)))
    }

    fn serialize_u16(self, value: u16) -> Result<Self::Ok> {
        Ok(Value::Uint64(u64::from(value)))
    }

    fn serialize_u32(self, value: u32) -> Result<Self::Ok> {
        Ok(Value::Uint64(u64::from(value)))
    }

    fn serialize_u64(self, value: u64) -> Result<Self::Ok> {
        Ok(Value::Uint64(value))
    }

    fn serialize_u128(self, value: u128) -> Result<Self::Ok> {
        u64::try_from(value).map(Value::Uint64).map_err(|_| {
            Error::EncodingError("unsigned JSON integer does not fit MMDB uint64".into())
        })
    }

    fn serialize_f32(self, value: f32) -> Result<Self::Ok> {
        Ok(Value::Double(f64::from(value)))
    }

    fn serialize_f64(self, value: f64) -> Result<Self::Ok> {
        Ok(Value::Double(value))
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok> {
        Ok(Value::Utf8(value.to_string()))
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok> {
        Ok(Value::Utf8(value.to_owned()))
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok> {
        Ok(Value::Array(
            value
                .iter()
                .map(|&byte| Value::Uint64(u64::from(byte)))
                .collect(),
        ))
    }

    fn serialize_none(self) -> Result<Self::Ok> {
        Err(null_error())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Self::Ok> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok> {
        Err(null_error())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok> {
        Err(null_error())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok> {
        Ok(Value::Utf8(variant.to_owned()))
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok> {
        let mut entries = BTreeMap::new();
        entries.insert(variant.to_owned(), value.serialize(ValueSerializer)?);
        Ok(Value::Map(entries))
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq> {
        Ok(SeqSerializer {
            values: Vec::with_capacity(len.unwrap_or(0)),
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple> {
        Ok(SeqSerializer {
            values: Vec::with_capacity(len),
        })
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        Ok(SeqSerializer {
            values: Vec::with_capacity(len),
        })
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        Ok(TupleVariantSerializer {
            name: variant,
            values: Vec::with_capacity(len),
        })
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap> {
        Ok(MapSerializer::default())
    }

    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct> {
        Ok(MapSerializer::default())
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        Ok(StructVariantSerializer {
            name: variant,
            entries: BTreeMap::new(),
        })
    }
}

impl ser::SerializeSeq for SeqSerializer {
    type Ok = Value;
    type Error = Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.values.push(value.serialize(ValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok> {
        Ok(Value::Array(self.values))
    }
}

impl ser::SerializeTuple for SeqSerializer {
    type Ok = Value;
    type Error = Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.values.push(value.serialize(ValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok> {
        Ok(Value::Array(self.values))
    }
}

impl ser::SerializeTupleStruct for SeqSerializer {
    type Ok = Value;
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.values.push(value.serialize(ValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok> {
        Ok(Value::Array(self.values))
    }
}

impl ser::SerializeTupleVariant for TupleVariantSerializer {
    type Ok = Value;
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.values.push(value.serialize(ValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok> {
        let mut entries = BTreeMap::new();
        entries.insert(self.name.to_owned(), Value::Array(self.values));
        Ok(Value::Map(entries))
    }
}

impl ser::SerializeMap for MapSerializer {
    type Ok = Value;
    type Error = Error;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<()> {
        self.pending_key = Some(key.serialize(KeySerializer)?);
        Ok(())
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        let key = self
            .pending_key
            .take()
            .ok_or_else(|| Error::EncodingError("MMDB map value without a key".into()))?;
        self.entries.insert(key, value.serialize(ValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok> {
        Ok(Value::Map(self.entries))
    }
}

impl ser::SerializeStruct for MapSerializer {
    type Ok = Value;
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.entries
            .insert(key.to_owned(), value.serialize(ValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok> {
        Ok(Value::Map(self.entries))
    }
}

impl ser::SerializeStructVariant for StructVariantSerializer {
    type Ok = Value;
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.entries
            .insert(key.to_owned(), value.serialize(ValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok> {
        let mut entries = BTreeMap::new();
        entries.insert(self.name.to_owned(), Value::Map(self.entries));
        Ok(Value::Map(entries))
    }
}

fn key_error() -> Error {
    Error::EncodingError("MMDB map keys must be strings".into())
}

impl ser::Serializer for KeySerializer {
    type Ok = String;
    type Error = Error;
    type SerializeSeq = Impossible<String, Error>;
    type SerializeTuple = Impossible<String, Error>;
    type SerializeTupleStruct = Impossible<String, Error>;
    type SerializeTupleVariant = Impossible<String, Error>;
    type SerializeMap = Impossible<String, Error>;
    type SerializeStruct = Impossible<String, Error>;
    type SerializeStructVariant = Impossible<String, Error>;

    fn serialize_str(self, value: &str) -> Result<Self::Ok> {
        Ok(value.to_owned())
    }

    fn collect_str<T: ?Sized + std::fmt::Display>(self, value: &T) -> Result<Self::Ok> {
        Ok(value.to_string())
    }

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_i8(self, _value: i8) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_i16(self, _value: i16) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_i32(self, _value: i32) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_i64(self, _value: i64) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_i128(self, _value: i128) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_u8(self, _value: u8) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_u16(self, _value: u16) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_u32(self, _value: u32) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_u64(self, _value: u64) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_u128(self, _value: u128) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_f32(self, _value: f32) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_f64(self, _value: f64) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_char(self, _value: char) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_none(self) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, _value: &T) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_unit(self) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok> {
        Err(key_error())
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq> {
        Err(key_error())
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple> {
        Err(key_error())
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        Err(key_error())
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        Err(key_error())
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap> {
        Err(key_error())
    }

    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct> {
        Err(key_error())
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        Err(key_error())
    }
}

#[cfg(test)]
mod serializer_tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Record {
        name: String,
        score: u32,
        signed: i32,
        nested: Vec<bool>,
    }

    #[derive(Serialize)]
    enum Shape {
        Unit,
        Newtype(u32),
        Tuple(u32, u32),
        Struct { x: i32 },
    }

    fn map(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
        )
    }

    #[test]
    fn serializes_struct_and_scalars() {
        let value = Value::from_serialize(&Record {
            name: "x".into(),
            score: 7,
            signed: -3,
            nested: vec![true, false],
        })
        .unwrap();
        assert_eq!(
            value,
            map([
                ("name", Value::Utf8("x".into())),
                ("score", Value::Uint64(7)),
                ("signed", Value::Int32(-3)),
                (
                    "nested",
                    Value::Array(vec![Value::Bool(true), Value::Bool(false)]),
                ),
            ])
        );
    }

    #[test]
    fn serializes_bytes_and_floats() {
        assert_eq!(
            Value::from_serialize(&serde_bytes::ByteBuf::from(vec![1u8, 2, 3])).unwrap(),
            Value::Array(vec![Value::Uint64(1), Value::Uint64(2), Value::Uint64(3)])
        );
        assert_eq!(Value::from_serialize(&1.5f64).unwrap(), Value::Double(1.5));
        assert_eq!(Value::from_serialize(&2.5f32).unwrap(), Value::Double(2.5));
    }

    #[test]
    fn serializes_number_boundaries() {
        assert_eq!(
            Value::from_serialize(&i64::from(i32::MIN)).unwrap(),
            Value::Int32(i32::MIN)
        );
        assert!(Value::from_serialize(&(i64::from(i32::MAX) + 1)).is_err());
        assert_eq!(
            Value::from_serialize(&u64::MAX).unwrap(),
            Value::Uint64(u64::MAX)
        );
        assert_eq!(
            Value::from_serialize(&(i128::from(i32::MAX) + 1)).unwrap(),
            Value::Uint64(2_147_483_648)
        );
        assert!(Value::from_serialize(&i128::MIN).is_err());
        assert_eq!(
            Value::from_serialize(&u128::from(u64::MAX)).unwrap(),
            Value::Uint64(u64::MAX)
        );
        assert!(Value::from_serialize(&(u128::from(u64::MAX) + 1)).is_err());
    }

    #[test]
    fn narrow_signed_numbers_chars_and_tuples_keep_their_values() {
        assert_eq!(Value::from_serialize(&(-5_i8)).unwrap(), Value::Int32(-5));
        assert_eq!(
            Value::from_serialize(&(-300_i16)).unwrap(),
            Value::Int32(-300)
        );
        assert_eq!(
            Value::from_serialize(&'é').unwrap(),
            Value::Utf8("é".into())
        );
        assert_eq!(
            Value::from_serialize(&(1_u32, 2_u32)).unwrap(),
            Value::Array(vec![Value::Uint64(1), Value::Uint64(2)])
        );
    }

    #[test]
    fn rejects_null_and_non_string_keys() {
        assert!(Value::from_serialize(&Option::<u32>::None).is_err());
        assert!(Value::from_serialize(&()).is_err());
        let mut keyed = BTreeMap::new();
        keyed.insert(1u32, 2u32);
        assert!(Value::from_serialize(&keyed).is_err());
        assert_eq!(
            Value::from_serialize(&Some(5u32)).unwrap(),
            Value::Uint64(5)
        );
    }

    fn sample_value_ref() -> Vec<ValueRef<'static>> {
        vec![
            ValueRef::Utf8("s"),
            ValueRef::Bytes(&[1, 2]),
            ValueRef::Double(1.5),
            ValueRef::Float(2.5),
            ValueRef::Uint16(16),
            ValueRef::Uint32(32),
            ValueRef::Int32(-3),
            ValueRef::Uint64(64),
            ValueRef::Uint128(u128::MAX),
            ValueRef::Bool(true),
            ValueRef::Array(vec![ValueRef::Uint64(1)]),
            ValueRef::Map(vec![("k", ValueRef::Int32(7))]),
        ]
    }

    #[test]
    fn value_ref_to_owned_value_covers_all_variants() {
        let owned: Vec<Value> = sample_value_ref()
            .iter()
            .map(ValueRef::to_owned_value)
            .collect();
        assert_eq!(
            owned,
            vec![
                Value::Utf8("s".into()),
                Value::Bytes(vec![1, 2]),
                Value::Double(1.5),
                Value::Float(2.5),
                Value::Uint16(16),
                Value::Uint32(32),
                Value::Int32(-3),
                Value::Uint64(64),
                Value::Uint128(u128::MAX),
                Value::Bool(true),
                Value::Array(vec![Value::Uint64(1)]),
                Value::Map([("k".to_owned(), Value::Int32(7))].into()),
            ]
        );
    }

    #[test]
    fn value_ref_to_json_covers_all_variants() {
        let json: Vec<serde_json::Value> =
            sample_value_ref().iter().map(ValueRef::to_json).collect();
        assert_eq!(
            json,
            vec![
                serde_json::json!("s"),
                serde_json::json!([1, 2]),
                serde_json::json!(1.5),
                serde_json::json!(2.5),
                serde_json::json!(16),
                serde_json::json!(32),
                serde_json::json!(-3),
                serde_json::json!(64),
                serde_json::json!(u128::MAX.to_string()),
                serde_json::json!(true),
                serde_json::json!([1]),
                serde_json::json!({"k": 7}),
            ]
        );
    }

    #[test]
    fn value_ref_get_requires_a_map() {
        let map = ValueRef::Map(vec![("k", ValueRef::Uint16(1))]);
        assert_eq!(map.get("k"), Some(&ValueRef::Uint16(1)));
        assert_eq!(map.get("missing"), None);
        assert_eq!(ValueRef::Uint64(1).get("k"), None);
    }

    #[test]
    fn serde_error_custom_builds_encoding_error() {
        let error: Error = <Error as serde::ser::Error>::custom("boom");
        let message = error.to_string();
        assert!(message.contains("boom"));
    }

    #[derive(Serialize)]
    struct Newtype(u64);

    #[derive(Serialize)]
    struct Tuple(u8, u16);

    #[test]
    fn serializes_newtype_and_tuple_struct_and_unit_struct() {
        assert_eq!(
            Value::from_serialize(&Newtype(9)).unwrap(),
            Value::Uint64(9)
        );
        assert_eq!(
            Value::from_serialize(&Tuple(1, 2)).unwrap(),
            Value::Array(vec![Value::Uint64(1), Value::Uint64(2)])
        );
        // A unit struct maps to the undefined null type and is rejected.
        #[derive(Serialize)]
        struct Unit;
        assert!(Value::from_serialize(&Unit).is_err());
    }

    #[test]
    fn key_serializer_accepts_strings_and_collect_str() {
        use serde::ser::Serializer;
        assert_eq!(
            KeySerializer.serialize_str("abc").unwrap(),
            "abc".to_owned()
        );
        assert_eq!(KeySerializer.collect_str(&7_u64).unwrap(), "7".to_owned());
    }

    #[test]
    fn key_serializer_rejects_non_string_keys() {
        use serde::ser::Serializer;
        macro_rules! reject {
            ($call:expr) => {
                assert!(matches!($call, Result::Err(_)), "expected key error")
            };
        }
        reject!(KeySerializer.serialize_bool(true));
        reject!(KeySerializer.serialize_i8(1));
        reject!(KeySerializer.serialize_i16(1));
        reject!(KeySerializer.serialize_i32(1));
        reject!(KeySerializer.serialize_i64(1));
        reject!(KeySerializer.serialize_i128(1));
        reject!(KeySerializer.serialize_u8(1));
        reject!(KeySerializer.serialize_u16(1));
        reject!(KeySerializer.serialize_u64(1));
        reject!(KeySerializer.serialize_u128(1));
        reject!(KeySerializer.serialize_f32(1.0));
        reject!(KeySerializer.serialize_f64(1.0));
        reject!(KeySerializer.serialize_char('x'));
        reject!(KeySerializer.serialize_bytes(&[1]));
        reject!(KeySerializer.serialize_none());
        reject!(KeySerializer.serialize_some(&1_u32));
        reject!(KeySerializer.serialize_unit());
        reject!(KeySerializer.serialize_unit_struct("U"));
        reject!(KeySerializer.serialize_unit_variant("E", 0, "V"));
        reject!(KeySerializer.serialize_newtype_variant("E", 0, "V", &1_u32));
        reject!(KeySerializer.serialize_seq(None));
        reject!(KeySerializer.serialize_tuple(1));
        reject!(KeySerializer.serialize_tuple_struct("T", 1));
        reject!(KeySerializer.serialize_tuple_variant("E", 0, "V", 1));
        reject!(KeySerializer.serialize_map(None));
        reject!(KeySerializer.serialize_struct("S", 1));
        reject!(KeySerializer.serialize_struct_variant("E", 0, "V", 1));
        let message = KeySerializer.serialize_bool(true).unwrap_err();
        assert!(matches!(
            message,
            crate::Error::EncodingError(msg) if msg.contains("strings")
        ));
    }

    #[test]
    fn serializes_enum_variants() {
        assert_eq!(
            Value::from_serialize(&Shape::Unit).unwrap(),
            Value::Utf8("Unit".into())
        );
        assert_eq!(
            Value::from_serialize(&Shape::Newtype(1)).unwrap(),
            map([("Newtype", Value::Uint64(1))])
        );
        assert_eq!(
            Value::from_serialize(&Shape::Tuple(1, 2)).unwrap(),
            map([(
                "Tuple",
                Value::Array(vec![Value::Uint64(1), Value::Uint64(2)])
            )])
        );
        assert_eq!(
            Value::from_serialize(&Shape::Struct { x: -1 }).unwrap(),
            map([("Struct", map([("x", Value::Int32(-1))]))])
        );
    }
}
