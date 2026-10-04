//! Traits used by derive macros and borrowed decoding.

use std::collections::BTreeMap;

use crate::decoder::RawDecoder;
use crate::{Error, Result, Value, ValueRef};

/// Trait implemented by `#[derive(MmdbDecode)]`.
///
/// Implement this trait manually for custom decoding behavior, or derive it
/// for a typed MMDB map. Borrowed fields must not outlive the source bytes.
pub trait MmdbDecode<'a>: Sized {
    /// Decodes one already-parsed MMDB value into `Self`.
    /// Returns a type-mismatch error if required fields are absent or invalid.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "derive")]
    /// # fn main() -> Result<(), libmaxminddb_rs::Error> {
    /// use libmaxminddb_rs::{MmdbDecode, ValueRef};
    /// #[derive(MmdbDecode)]
    /// struct Record<'a> { category: &'a str }
    /// let value = ValueRef::Map(vec![("category", ValueRef::Utf8("example"))]);
    /// let record = Record::decode(&value)?;
    /// assert_eq!(record.category, "example");
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "derive"))] fn main() {}
    /// ```
    fn decode(value: &ValueRef<'a>) -> Result<Self>;

    /// Decodes `Self` straight from the encoded record under `decoder`.
    ///
    /// `Reader::lookup_borrowed` calls this method. The derive overrides it
    /// with an allocation-free, single-pass decoder; the default keeps
    /// hand-written implementations working by materializing a [`ValueRef`]
    /// and delegating to [`MmdbDecode::decode`].
    #[doc(hidden)]
    #[inline]
    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        let value = decoder.read_value()?;
        Self::decode(&value)
    }
}

/// Trait implemented by `#[derive(MmdbEncode)]`.
pub trait MmdbEncode {
    /// Encodes `Self` into the writer's owned MMDB value model.
    /// Returns an encoding error for an unsupported field value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "derive")]
    /// # fn main() -> Result<(), libmaxminddb_rs::Error> {
    /// use libmaxminddb_rs::{MmdbEncode, Value};
    /// #[derive(MmdbEncode)]
    /// struct Record { asn: u32 }
    /// let value = Record { asn: 64512 }.encode()?;
    /// assert!(matches!(value, Value::Map(_)));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "derive"))] fn main() {}
    /// ```
    fn encode(&self) -> Result<Value>;
}

/// Input accepted by editor updates: an owned [`Value`] or a borrowed record.
///
/// Owned values are moved without cloning. Pass `&record` for structs that
/// implement [`MmdbEncode`], including `#[derive(MmdbEncode)]` structs.
pub trait IntoMmdbValue {
    /// Moves or encodes this input into the owned MMDB value model.
    fn into_mmdb_value(self) -> Result<Value>;
}

impl IntoMmdbValue for Value {
    fn into_mmdb_value(self) -> Result<Value> {
        Ok(self)
    }
}

impl<T: MmdbEncode + ?Sized> IntoMmdbValue for &T {
    fn into_mmdb_value(self) -> Result<Value> {
        self.encode()
    }
}

/// Trait for a custom record that carries its own network/CIDR.
///
/// `#[derive(MmdbRecord)]` implements this trait when one field is annotated with
/// `#[mmdb(network)]`. Pair it with `#[derive(MmdbEncode)]`; the network field is then
/// excluded from the encoded MMDB payload.
pub trait MmdbRecord: MmdbEncode {
    /// Returns the network to which this record should be attached.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "derive")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use libmaxminddb_rs::{IpNetwork, MmdbEncode, MmdbRecord};
    /// #[derive(MmdbEncode, MmdbRecord)]
    /// struct Record { #[mmdb(network)] network: IpNetwork, asn: u32 }
    /// let record = Record { network: "198.51.100.0/24".parse()?, asn: 64512 };
    /// assert_eq!(record.network().to_string(), "198.51.100.0/24");
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "derive"))] fn main() {}
    /// ```
    fn network(&self) -> crate::IpNetwork;
}

/// Internal/public field decoder used by generated implementations.
pub trait DecodeField<'a>: Sized {
    /// Decodes an optional map field into this field type.
    /// Returns an error when a required field is absent or has the wrong type.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{DecodeField, ValueRef};
    /// let value = ValueRef::Utf8("example");
    /// let category = <&str>::decode_field(Some(&value))?;
    /// assert_eq!(category, "example");
    /// # Ok::<(), libmaxminddb_rs::Error>(())
    /// ```
    fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self>;

    /// Decodes a present field straight from the encoded value under
    /// `decoder`, without building a [`ValueRef`].
    ///
    /// The built-in implementations never allocate (except `String`, `Vec`
    /// and `BTreeMap`, which own their contents). The default materializes a
    /// [`ValueRef`] and delegates to [`DecodeField::decode_field`].
    #[doc(hidden)]
    #[inline]
    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        let value = decoder.read_value()?;
        Self::decode_field(Some(&value))
    }

    /// Value for a field that is absent from the record.
    #[doc(hidden)]
    #[inline]
    fn decode_missing() -> Result<Self> {
        Self::decode_field(None)
    }
}

/// Internal/public field encoder used by generated implementations.
pub trait EncodeField {
    /// Encodes one struct field into an MMDB value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{EncodeField, Value};
    /// assert_eq!(64512_u32.encode_field()?, Value::Uint32(64512));
    /// # Ok::<(), libmaxminddb_rs::Error>(())
    /// ```
    fn encode_field(&self) -> Result<Value>;

    /// Encodes a field for insertion into a struct map.
    ///
    /// Most field types return `Some(value)`. `Option<T>` overrides this hook so `None`
    /// omits the field entirely, matching MMDB's lack of a null data type.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::EncodeField;
    /// assert!(Option::<u32>::None.encode_optional_field()?.is_none());
    /// # Ok::<(), libmaxminddb_rs::Error>(())
    /// ```
    fn encode_optional_field(&self) -> Result<Option<Value>> {
        self.encode_field().map(Some)
    }
}

impl<'a> DecodeField<'a> for &'a str {
    fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
        match value {
            Some(ValueRef::Utf8(v)) => Ok(*v),
            _ => Err(Error::DecodingError("expected UTF-8 string".into())),
        }
    }

    #[inline]
    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        decoder.read_str()
    }
}

impl<'a> DecodeField<'a> for &'a [u8] {
    fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
        match value {
            Some(ValueRef::Bytes(v)) => Ok(*v),
            _ => Err(Error::DecodingError("expected byte array".into())),
        }
    }

    #[inline]
    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        decoder.read_bytes()
    }
}

impl<'a> DecodeField<'a> for String {
    fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
        <&str as DecodeField<'a>>::decode_field(value).map(ToOwned::to_owned)
    }

    #[inline]
    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        decoder.read_str().map(ToOwned::to_owned)
    }
}

impl<'a, T: DecodeField<'a>> DecodeField<'a> for Option<T> {
    fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
        value.map(|v| T::decode_field(Some(v))).transpose()
    }

    #[inline]
    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        T::decode_raw(decoder).map(Some)
    }

    #[inline]
    fn decode_missing() -> Result<Self> {
        Ok(None)
    }
}

impl<'a, T: DecodeField<'a>> DecodeField<'a> for Vec<T> {
    fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
        match value {
            Some(ValueRef::Array(values)) => values
                .iter()
                .map(|value| T::decode_field(Some(value)))
                .collect(),
            _ => Err(Error::DecodingError("expected array".into())),
        }
    }

    #[inline]
    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        let array = decoder.enter_array("expected array")?;
        let mut values = Vec::with_capacity(decoder.capacity_hint(array));
        for _ in 0..array.len() {
            values.push(T::decode_raw(decoder)?);
        }
        decoder.leave(array);
        Ok(values)
    }
}

impl<'a, T: DecodeField<'a>> DecodeField<'a> for BTreeMap<String, T> {
    fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
        match value {
            Some(ValueRef::Map(values)) => values
                .iter()
                .map(|(key, value)| Ok(((*key).to_owned(), T::decode_field(Some(value))?)))
                .collect(),
            _ => Err(Error::DecodingError("expected map".into())),
        }
    }

    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        let map = decoder.enter_map("expected map")?;
        let mut entries = BTreeMap::new();
        for _ in 0..map.len() {
            let key = decoder.read_key_str()?.to_owned();
            let value = T::decode_raw(decoder)?;
            // `collect` into a BTreeMap keeps the last duplicate; match it.
            entries.insert(key, value);
        }
        decoder.leave(map);
        Ok(entries)
    }
}

macro_rules! decode_num {
    ($ty:ty, $($variant:ident),+ $(,)?) => {
        impl<'a> DecodeField<'a> for $ty {
            fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
                match value {
                    $(Some(ValueRef::$variant(v)) => <$ty>::try_from(*v).map_err(|_| Error::DecodingError("numeric conversion failed".into())),)+
                    _ => Err(Error::DecodingError("expected numeric value".into())),
                }
            }

            #[inline]
            fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
                let value = decoder.read_u64()?;
                <$ty>::try_from(value).map_err(|_| conversion_error())
            }
        }
    };
}

#[cold]
#[inline(never)]
fn conversion_error() -> Error {
    Error::DecodingError("numeric conversion failed".into())
}

decode_num!(u16, Uint16, Uint32, Uint64);
decode_num!(u32, Uint16, Uint32, Uint64);
decode_num!(u64, Uint16, Uint32, Uint64);

impl<'a> DecodeField<'a> for u128 {
    fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
        match value {
            Some(ValueRef::Uint16(v)) => Ok((*v).into()),
            Some(ValueRef::Uint32(v)) => Ok((*v).into()),
            Some(ValueRef::Uint64(v)) => Ok((*v).into()),
            Some(ValueRef::Uint128(v)) => Ok(*v),
            _ => Err(Error::DecodingError("expected unsigned integer".into())),
        }
    }

    #[inline]
    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        decoder.read_u128()
    }
}

impl<'a> DecodeField<'a> for i32 {
    fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
        match value {
            Some(ValueRef::Int32(v)) => Ok(*v),
            _ => Err(Error::DecodingError("expected int32".into())),
        }
    }

    #[inline]
    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        decoder.read_i32()
    }
}

impl<'a> DecodeField<'a> for f64 {
    fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
        match value {
            Some(ValueRef::Double(v)) => Ok(*v),
            Some(ValueRef::Float(v)) => Ok(f64::from(*v)),
            _ => Err(Error::DecodingError("expected float/double".into())),
        }
    }

    #[inline]
    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        decoder.read_f64()
    }
}

impl<'a> DecodeField<'a> for f32 {
    fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
        match value {
            Some(ValueRef::Float(v)) => Ok(*v),
            _ => Err(Error::DecodingError("expected float".into())),
        }
    }

    #[inline]
    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        decoder.read_f32()
    }
}

impl<'a> DecodeField<'a> for bool {
    fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
        match value {
            Some(ValueRef::Bool(v)) => Ok(*v),
            _ => Err(Error::DecodingError("expected boolean".into())),
        }
    }

    #[inline]
    fn decode_raw(decoder: &mut RawDecoder<'a>) -> Result<Self> {
        decoder.read_bool()
    }
}

impl<T: EncodeField> EncodeField for Option<T> {
    fn encode_field(&self) -> Result<Value> {
        self.as_ref()
            .map(EncodeField::encode_field)
            .transpose()?
            .ok_or_else(|| Error::EncodingError("MMDB has no null value".into()))
    }

    fn encode_optional_field(&self) -> Result<Option<Value>> {
        self.as_ref().map(EncodeField::encode_field).transpose()
    }
}

impl EncodeField for str {
    fn encode_field(&self) -> Result<Value> {
        Ok(Value::Utf8(self.to_owned()))
    }
}
impl EncodeField for &str {
    fn encode_field(&self) -> Result<Value> {
        Ok(Value::Utf8((*self).to_owned()))
    }
}
impl EncodeField for String {
    fn encode_field(&self) -> Result<Value> {
        Ok(Value::Utf8(self.clone()))
    }
}
impl EncodeField for bool {
    fn encode_field(&self) -> Result<Value> {
        Ok(Value::Bool(*self))
    }
}
impl EncodeField for f32 {
    fn encode_field(&self) -> Result<Value> {
        Ok(Value::Float(*self))
    }
}
impl EncodeField for f64 {
    fn encode_field(&self) -> Result<Value> {
        Ok(Value::Double(*self))
    }
}
macro_rules! encode_int {
    ($ty:ty, $variant:ident) => {
        impl EncodeField for $ty {
            fn encode_field(&self) -> Result<Value> {
                Ok(Value::$variant(*self as _))
            }
        }
    };
}
encode_int!(u16, Uint16);
encode_int!(u32, Uint32);
encode_int!(u64, Uint64);
encode_int!(u128, Uint128);
encode_int!(i32, Int32);

impl<T: EncodeField> EncodeField for Vec<T> {
    fn encode_field(&self) -> Result<Value> {
        Ok(Value::Array(
            self.iter()
                .map(EncodeField::encode_field)
                .collect::<Result<_>>()?,
        ))
    }
}

impl<T: EncodeField> EncodeField for [T] {
    fn encode_field(&self) -> Result<Value> {
        Ok(Value::Array(
            self.iter()
                .map(EncodeField::encode_field)
                .collect::<Result<_>>()?,
        ))
    }
}

impl<T: EncodeField> EncodeField for BTreeMap<String, T> {
    fn encode_field(&self) -> Result<Value> {
        Ok(Value::Map(
            self.iter()
                .map(|(k, v)| Ok((k.clone(), v.encode_field()?)))
                .collect::<Result<_>>()?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn string_and_byte_fields_decode_and_reject_wrong_types() {
        assert_eq!(
            <&str as DecodeField<'_>>::decode_field(Some(&ValueRef::Utf8("x"))).unwrap(),
            "x"
        );
        assert!(<&str as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint32(1))).is_err());
        assert!(<&str as DecodeField<'_>>::decode_field(None).is_err());

        assert_eq!(
            <String as DecodeField<'_>>::decode_field(Some(&ValueRef::Utf8("x"))).unwrap(),
            "x"
        );
        assert!(<String as DecodeField<'_>>::decode_field(None).is_err());

        assert_eq!(
            <&[u8] as DecodeField<'_>>::decode_field(Some(&ValueRef::Bytes(&[1, 2]))).unwrap(),
            &[1, 2]
        );
        assert!(<&[u8] as DecodeField<'_>>::decode_field(Some(&ValueRef::Utf8("x"))).is_err());
    }

    #[test]
    fn option_fields_delegate_and_transpose_errors() {
        assert_eq!(
            <Option<&str> as DecodeField<'_>>::decode_field(Some(&ValueRef::Utf8("x"))).unwrap(),
            Some("x")
        );
        assert_eq!(
            <Option<&str> as DecodeField<'_>>::decode_field(None).unwrap(),
            None
        );
        assert!(
            <Option<&str> as DecodeField<'_>>::decode_field(Some(&ValueRef::Bool(true))).is_err()
        );
    }

    #[test]
    fn vec_and_map_fields_decode_and_reject() {
        let array = ValueRef::Array(vec![ValueRef::Uint16(1), ValueRef::Uint16(2)]);
        assert_eq!(
            <Vec<u16> as DecodeField<'_>>::decode_field(Some(&array)).unwrap(),
            vec![1, 2]
        );
        let bad = ValueRef::Array(vec![ValueRef::Uint16(1), ValueRef::Utf8("x")]);
        assert!(<Vec<u16> as DecodeField<'_>>::decode_field(Some(&bad)).is_err());
        assert!(<Vec<u16> as DecodeField<'_>>::decode_field(Some(&ValueRef::Utf8("x"))).is_err());

        let map = ValueRef::Map(vec![("a", ValueRef::Utf8("x"))]);
        let decoded =
            <BTreeMap<String, String> as DecodeField<'_>>::decode_field(Some(&map)).unwrap();
        assert_eq!(decoded.get("a").map(String::as_str), Some("x"));
        let bad_map = ValueRef::Map(vec![("a", ValueRef::Bool(true))]);
        assert!(
            <BTreeMap<String, String> as DecodeField<'_>>::decode_field(Some(&bad_map)).is_err()
        );
        assert!(
            <BTreeMap<String, String> as DecodeField<'_>>::decode_field(Some(&ValueRef::Array(
                vec![]
            )))
            .is_err()
        );
    }

    #[test]
    fn numeric_fields_decode_and_report_conversion_failures() {
        assert_eq!(
            <u16 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint16(7))).unwrap(),
            7
        );
        assert_eq!(
            <u16 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint32(7))).unwrap(),
            7
        );
        assert_eq!(
            <u16 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint64(7))).unwrap(),
            7
        );
        assert!(matches!(
            <u16 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint32(70_000))),
            Err(Error::DecodingError(_))
        ));
        assert!(<u16 as DecodeField<'_>>::decode_field(Some(&ValueRef::Utf8("x"))).is_err());

        assert_eq!(
            <u32 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint16(3))).unwrap(),
            3
        );
        assert_eq!(
            <u32 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint32(3))).unwrap(),
            3
        );
        assert_eq!(
            <u32 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint64(3))).unwrap(),
            3
        );
        assert!(<u32 as DecodeField<'_>>::decode_field(Some(&ValueRef::Int32(3))).is_err());

        assert_eq!(
            <u64 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint16(3))).unwrap(),
            3
        );
        assert_eq!(
            <u64 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint32(3))).unwrap(),
            3
        );
        assert_eq!(
            <u64 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint64(3))).unwrap(),
            3
        );
        assert!(<u64 as DecodeField<'_>>::decode_field(Some(&ValueRef::Int32(3))).is_err());

        assert_eq!(
            <u128 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint16(1))).unwrap(),
            1_u128
        );
        assert_eq!(
            <u128 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint32(2))).unwrap(),
            2_u128
        );
        assert_eq!(
            <u128 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint64(3))).unwrap(),
            3_u128
        );
        assert_eq!(
            <u128 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint128(4))).unwrap(),
            4_u128
        );
        assert!(<u128 as DecodeField<'_>>::decode_field(Some(&ValueRef::Bool(true))).is_err());

        assert_eq!(
            <i32 as DecodeField<'_>>::decode_field(Some(&ValueRef::Int32(-5))).unwrap(),
            -5
        );
        assert!(<i32 as DecodeField<'_>>::decode_field(Some(&ValueRef::Uint16(5))).is_err());
    }

    #[test]
    fn float_double_and_bool_fields_decode_and_reject() {
        assert_eq!(
            <f64 as DecodeField<'_>>::decode_field(Some(&ValueRef::Double(1.5))).unwrap(),
            1.5
        );
        assert_eq!(
            <f64 as DecodeField<'_>>::decode_field(Some(&ValueRef::Float(0.5))).unwrap(),
            0.5
        );
        assert!(<f64 as DecodeField<'_>>::decode_field(Some(&ValueRef::Bool(true))).is_err());

        assert_eq!(
            <f32 as DecodeField<'_>>::decode_field(Some(&ValueRef::Float(0.5))).unwrap(),
            0.5
        );
        assert!(<f32 as DecodeField<'_>>::decode_field(Some(&ValueRef::Double(0.5))).is_err());

        assert!(<bool as DecodeField<'_>>::decode_field(Some(&ValueRef::Bool(true))).unwrap());
        assert!(<bool as DecodeField<'_>>::decode_field(Some(&ValueRef::Utf8("x"))).is_err());
    }

    #[test]
    fn encode_fields_produce_expected_values() {
        assert_eq!("abc".encode_field().unwrap(), Value::Utf8("abc".into()));
        assert_eq!(
            String::from("abc").encode_field().unwrap(),
            Value::Utf8("abc".into())
        );
        assert_eq!(
            <str as EncodeField>::encode_field("abc").unwrap(),
            Value::Utf8("abc".into())
        );
        assert_eq!(true.encode_field().unwrap(), Value::Bool(true));
        assert_eq!(1.5f32.encode_field().unwrap(), Value::Float(1.5));
        assert_eq!(1.5f64.encode_field().unwrap(), Value::Double(1.5));
        assert_eq!(7u16.encode_field().unwrap(), Value::Uint16(7));
        assert_eq!(7u32.encode_field().unwrap(), Value::Uint32(7));
        assert_eq!(7u64.encode_field().unwrap(), Value::Uint64(7));
        assert_eq!(7u128.encode_field().unwrap(), Value::Uint128(7));
        assert_eq!((-7i32).encode_field().unwrap(), Value::Int32(-7));

        assert_eq!(
            vec![1u16, 2].encode_field().unwrap(),
            Value::Array(vec![Value::Uint16(1), Value::Uint16(2)])
        );
        let slice: &[u16] = &[3, 4];
        assert_eq!(
            slice.encode_field().unwrap(),
            Value::Array(vec![Value::Uint16(3), Value::Uint16(4)])
        );
        let mut map = BTreeMap::new();
        map.insert("a".to_string(), 5u32);
        assert_eq!(
            map.encode_field().unwrap(),
            Value::Map(BTreeMap::from([("a".to_string(), Value::Uint32(5))]))
        );
    }

    #[test]
    fn option_encode_fields_and_default_optional_hook() {
        assert_eq!(Some(7u16).encode_field().unwrap(), Value::Uint16(7));
        assert!(None::<u16>.encode_field().is_err());
        assert_eq!(
            Some(7u16).encode_optional_field().unwrap(),
            Some(Value::Uint16(7))
        );
        assert_eq!(None::<u16>.encode_optional_field().unwrap(), None);
        assert_eq!(
            5u16.encode_optional_field().unwrap(),
            Some(Value::Uint16(5))
        );
    }

    #[cfg(feature = "writer")]
    #[test]
    fn raw_field_decoders_cover_supported_wire_types_and_conversion_errors() {
        use crate::{decoder::RawDecoder, encoder::encode_value};

        macro_rules! check_raw {
            ($ty:ty, $value:expr, $expected:expr) => {{
                let mut bytes = Vec::new();
                encode_value(&$value, &mut bytes).unwrap();
                let mut decoder = RawDecoder::new(&bytes, 0, bytes.len(), 0);
                assert_eq!(
                    <$ty as DecodeField<'_>>::decode_raw(&mut decoder).unwrap(),
                    $expected
                );
            }};
        }
        check_raw!(&str, Value::Utf8("borrowed".into()), "borrowed");
        check_raw!(String, Value::Utf8("owned".into()), "owned");
        check_raw!(&[u8], Value::Bytes(vec![1, 2]), &[1, 2]);
        check_raw!(Option<u32>, Value::Uint32(7), Some(7));
        check_raw!(u16, Value::Uint32(7), 7);
        check_raw!(u32, Value::Uint32(7), 7);
        check_raw!(u64, Value::Uint64(1 << 40), 1 << 40);
        check_raw!(u128, Value::Uint128(1 << 100), 1 << 100);
        check_raw!(i32, Value::Int32(-7), -7);
        check_raw!(f32, Value::Float(1.5), 1.5);
        check_raw!(f64, Value::Double(1.5), 1.5);
        check_raw!(bool, Value::Bool(true), true);
        check_raw!(
            Vec<u16>,
            Value::Array(vec![Value::Uint16(1), Value::Uint16(2)]),
            vec![1, 2]
        );
        check_raw!(
            BTreeMap<String, u32>,
            Value::Map(BTreeMap::from([("asn".into(), Value::Uint32(7))])),
            BTreeMap::from([("asn".into(), 7)])
        );

        let mut bytes = Vec::new();
        encode_value(&Value::Uint64(70_000), &mut bytes).unwrap();
        let mut decoder = RawDecoder::new(&bytes, 0, bytes.len(), 0);
        assert!(<u16 as DecodeField<'_>>::decode_raw(&mut decoder).is_err());
        assert!(<u32 as DecodeField<'_>>::decode_missing().is_err());
        assert_eq!(
            <Option<u32> as DecodeField<'_>>::decode_missing().unwrap(),
            None
        );
    }

    #[cfg(feature = "writer")]
    #[test]
    fn default_field_decoder_materializes_custom_fields() {
        use crate::{decoder::RawDecoder, encoder::encode_value};

        struct Positive(u32);
        impl<'a> DecodeField<'a> for Positive {
            fn decode_field(value: Option<&ValueRef<'a>>) -> Result<Self> {
                match value {
                    Some(ValueRef::Uint32(n)) if *n > 0 => Ok(Self(*n)),
                    _ => Err(Error::DecodingError("positive number required".into())),
                }
            }
        }

        let mut bytes = Vec::new();
        encode_value(&Value::Uint32(7), &mut bytes).unwrap();
        let mut decoder = RawDecoder::new(&bytes, 0, bytes.len(), 0);
        assert_eq!(Positive::decode_raw(&mut decoder).unwrap().0, 7);
        assert!(Positive::decode_missing().is_err());
    }
}
