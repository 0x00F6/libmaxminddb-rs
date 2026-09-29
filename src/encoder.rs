//! MMDB data encoder.

use crate::{Error, Result, Value};

/// Encodes one owned MMDB value.
pub(crate) fn encode_value(value: &Value, out: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Utf8(v) => {
            control(2, v.len(), out)?;
            out.extend_from_slice(v.as_bytes());
        }
        Value::Bytes(v) => {
            control(4, v.len(), out)?;
            out.extend_from_slice(v);
        }
        Value::Double(v) => {
            control(3, 8, out)?;
            out.extend_from_slice(&v.to_be_bytes());
        }
        Value::Float(v) => {
            control(15, 4, out)?;
            out.extend_from_slice(&v.to_be_bytes());
        }
        Value::Uint16(v) => encode_uint(5, &v.to_be_bytes(), out)?,
        Value::Uint32(v) => encode_uint(6, &v.to_be_bytes(), out)?,
        Value::Int32(v) => {
            let bytes = v.to_be_bytes();
            let payload = if *v < 0 {
                &bytes[..]
            } else {
                trim_unsigned(&bytes)
            };
            control(8, payload.len(), out)?;
            out.extend_from_slice(payload);
        }
        Value::Uint64(v) => encode_uint(9, &v.to_be_bytes(), out)?,
        Value::Uint128(v) => encode_uint(10, &v.to_be_bytes(), out)?,
        Value::Bool(v) => control(14, usize::from(*v), out)?,
        Value::Array(values) => {
            control(11, values.len(), out)?;
            for value in values {
                encode_value(value, out)?;
            }
        }
        Value::Map(values) => {
            control(7, values.len(), out)?;
            for (key, value) in values {
                control(2, key.len(), out)?;
                out.extend_from_slice(key.as_bytes());
                encode_value(value, out)?;
            }
        }
    }
    Ok(())
}

fn encode_uint(data_type: u8, bytes: &[u8], out: &mut Vec<u8>) -> Result<()> {
    let payload = trim_unsigned(bytes);
    control(data_type, payload.len(), out)?;
    out.extend_from_slice(payload);
    Ok(())
}

fn trim_unsigned(bytes: &[u8]) -> &[u8] {
    let pos = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    &bytes[pos..]
}

fn control(data_type: u8, size: usize, out: &mut Vec<u8>) -> Result<()> {
    let (size_code, extra, len) = size_bytes(size)?;
    if data_type <= 7 {
        out.push((data_type << 5) | size_code);
    } else {
        out.push(size_code);
        out.push(
            data_type
                .checked_sub(7)
                .ok_or(Error::InvalidDataType(data_type))?,
        );
    }
    out.extend_from_slice(&extra[..len]);
    Ok(())
}

fn size_bytes(size: usize) -> Result<(u8, [u8; 3], usize)> {
    match size {
        0..=28 => Ok((size as u8, [0; 3], 0)),
        29..=284 => Ok((29, [(size - 29) as u8, 0, 0], 1)),
        285..=65_820 => {
            let bytes = ((size - 285) as u16).to_be_bytes();
            Ok((30, [bytes[0], bytes[1], 0], 2))
        }
        65_821..=16_843_036 => {
            let n = size - 65_821;
            Ok((31, [(n >> 16) as u8, (n >> 8) as u8, n as u8], 3))
        }
        _ => Err(Error::EncodingError(
            "MMDB field or container exceeds format limit".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_boundaries_have_exact_wire_bytes() {
        for (size, expected) in [
            (0, vec![0x40]),
            (28, vec![0x5c]),
            (29, vec![0x5d, 0]),
            (284, vec![0x5d, 255]),
            (285, vec![0x5e, 0, 0]),
            (65_820, vec![0x5e, 255, 255]),
            (65_821, vec![0x5f, 0, 0, 0]),
            (16_843_036, vec![0x5f, 255, 255, 255]),
        ] {
            let mut out = Vec::new();
            control(2, size, &mut out).unwrap();
            assert_eq!(out, expected);
        }
        assert!(control(2, 16_843_037, &mut Vec::new()).is_err());
    }

    #[test]
    fn oversized_payloads_and_map_keys_are_rejected_before_writing() {
        let oversized = "x".repeat(16_843_037);
        let cases = [
            Value::Utf8(oversized.clone()),
            Value::Bytes(oversized.as_bytes().to_vec()),
            Value::Array(vec![Value::Utf8(oversized.clone())]),
            Value::Map(std::collections::BTreeMap::from([(
                "field".into(),
                Value::Utf8(oversized.clone()),
            )])),
            Value::Map(std::collections::BTreeMap::from([(
                oversized,
                Value::Bool(true),
            )])),
        ];
        for value in cases {
            let mut out = Vec::new();
            assert!(matches!(
                encode_value(&value, &mut out),
                Err(Error::EncodingError(_))
            ));
            assert!(out.len() < 16_843_037);
        }
    }
}
