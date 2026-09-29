//! Schema-directed, allocation-free decoding for typed lookups.
//!
//! [`Decoder::decode_at`] materializes a generic [`ValueRef`] tree, which costs
//! one `Vec` per map and array (13 allocations for a GeoIP2-City record). When
//! the caller already knows the shape it wants, as `#[derive(MmdbDecode)]`
//! structs do, that tree is pure overhead. [`RawDecoder`] walks the encoded
//! bytes exactly once instead:
//!
//! - borrowed scalars (`&str`, `&[u8]`, numbers) go straight into the
//!   destination fields;
//! - map keys are compared as raw bytes against the field names, so matching
//!   needs neither a `ValueRef` nor UTF-8 validation (a byte-equal key is
//!   necessarily the valid UTF-8 field name);
//! - unknown entries are skipped structurally with an iterative loop that never
//!   follows pointers, so skipping is linear in the bytes it crosses and needs
//!   no recursion.
//!
//! The hardening of the generic decoder is preserved: every read is
//! bounds-checked, pointer chains and container nesting share the
//! `MAX_DEPTH` limit, and every visited value, decoded or skipped, is charged to
//! the same value/byte budget. Skipped payloads are validated structurally
//! (type tags, size limits, bounds) but not interpreted, because they never
//! reach the caller.

use super::{DecodeBudget, Decoder, MAX_CONTAINER_ITEMS, MAX_DEPTH, utf8};
use crate::{Error, Result, ValueRef};

/// `resume` sentinel: the value was stored inline, continue after its payload.
const INLINE: usize = usize::MAX;

/// MMDB type tags used by the typed readers.
const T_UTF8: u8 = 2;
const T_DOUBLE: u8 = 3;
const T_BYTES: u8 = 4;
const T_U16: u8 = 5;
const T_U32: u8 = 6;
const T_MAP: u8 = 7;
const T_I32: u8 = 8;
const T_U64: u8 = 9;
const T_U128: u8 = 10;
const T_ARRAY: u8 = 11;
const T_BOOL: u8 = 14;
const T_FLOAT: u8 = 15;

/// Cursor that decodes MMDB values directly into caller-chosen Rust types.
///
/// This type is an implementation detail of `#[derive(MmdbDecode)]` and of the
/// built-in [`DecodeField`](crate::DecodeField) implementations; it is public
/// only so that generated code can name it.
#[doc(hidden)]
pub struct RawDecoder<'a> {
    dec: Decoder<'a>,
    /// Absolute offset of the next byte to read.
    pos: usize,
    /// Current container nesting (pointer hops are bounded against it too).
    depth: usize,
    budget: DecodeBudget,
}

/// An open map or array returned by [`RawDecoder::enter_map`] /
/// [`RawDecoder::enter_array`].
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct Container {
    len: usize,
    /// Cursor position to restore once the container is left when it was
    /// reached through a pointer; [`INLINE`] otherwise.
    resume: usize,
}

impl Container {
    /// Number of entries (map) or elements (array).
    #[inline(always)]
    pub fn len(self) -> usize {
        self.len
    }

    /// Returns `true` for an empty container.
    #[inline(always)]
    pub fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// Builds a decoding error without putting the `String` construction on the
/// success path of the (inlined) callers.
#[cold]
#[inline(never)]
fn mismatch(message: &'static str) -> Error {
    Error::DecodingError(message.into())
}

#[cold]
#[inline(never)]
fn eof() -> Error {
    Error::UnexpectedEof
}

#[cold]
#[inline(never)]
fn too_deep() -> Error {
    Error::ResourceLimit("maximum MMDB nesting depth exceeded")
}

impl<'a> RawDecoder<'a> {
    /// Positions a decoder on the value at absolute offset `offset`.
    ///
    /// `pointer_base` is the start of the data section and `limit` the first
    /// byte that data values may not reach (the metadata marker).
    #[inline]
    pub(crate) fn new(data: &'a [u8], pointer_base: usize, limit: usize, offset: usize) -> Self {
        Self {
            dec: Decoder::new(data, pointer_base, limit),
            pos: offset,
            depth: 0,
            budget: DecodeBudget::default(),
        }
    }

    #[inline(always)]
    fn byte(&self, pos: usize) -> Result<u8> {
        match self.dec.data.get(pos) {
            Some(&byte) => Ok(byte),
            None => Err(eof()),
        }
    }

    /// Resolves the header of the value under the cursor.
    ///
    /// Returns `(type, size, resume)` and leaves `pos` on the first payload
    /// byte. Pointers are followed transparently; `resume` then holds the
    /// offset just after the pointer, where the caller continues once the
    /// value has been consumed.
    #[inline(always)]
    fn header(&mut self) -> Result<(u8, usize, usize)> {
        self.budget.charge_value()?;
        let mut pos = self.pos;
        let mut control = self.byte(pos)?;
        let mut resume = INLINE;
        if control >> 5 == 1 {
            (pos, control, resume) = self.follow(pos, control)?;
        }
        pos += 1;
        let mut kind = control >> 5;
        if kind == 0 {
            let ext = self.byte(pos)?;
            pos += 1;
            // 249..=255 would overflow `ext + 7`; they are reserved anyway.
            if ext > 248 {
                return Err(Error::InvalidDataType(ext));
            }
            kind = ext + 7;
        }
        let (size, extra) = self.dec.decode_size(control & 0x1f, pos)?;
        self.pos = pos + extra;
        Ok((kind, size, resume))
    }

    /// Follows the pointer chain whose first control byte sits at `pos`.
    ///
    /// Returns the target offset, the target's control byte and the resume
    /// offset. Every hop is charged like a decoded value and counted against
    /// the nesting limit, which bounds malicious pointer cycles exactly like
    /// the generic decoder does.
    #[inline(never)]
    fn follow(&mut self, pos: usize, control: u8) -> Result<(usize, u8, usize)> {
        let (mut pointer, consumed) = self.dec.decode_pointer(control, pos + 1)?;
        let resume = pos + 1 + consumed;
        let mut hops = 0_usize;
        loop {
            hops += 1;
            if self.depth + hops > MAX_DEPTH {
                return Err(too_deep());
            }
            self.budget.charge_value()?;
            let target = self
                .dec
                .pointer_base
                .checked_add(pointer)
                .ok_or(Error::InvalidOffset(pointer))?;
            let control = self.byte(target)?;
            if control >> 5 != 1 {
                return Ok((target, control, resume));
            }
            pointer = self.dec.decode_pointer(control, target + 1)?.0;
        }
    }

    /// Takes `size` payload bytes at the cursor and moves past the value.
    #[inline(always)]
    fn payload(&mut self, size: usize, resume: usize) -> Result<&'a [u8]> {
        let start = self.pos;
        if !self.dec.in_range(start, size) {
            return Err(eof());
        }
        self.pos = if resume == INLINE {
            start + size
        } else {
            resume
        };
        Ok(self.dec.slice(start, size))
    }

    /// Moves past a payload-less value (booleans).
    #[inline(always)]
    fn done(&mut self, resume: usize) {
        if resume != INLINE {
            self.pos = resume;
        }
    }

    /// Reads a big-endian unsigned payload of `size <= 8` bytes.
    #[inline(always)]
    fn uint_payload(&mut self, size: usize, resume: usize) -> Result<u64> {
        debug_assert!(size <= 8, "callers validate integer widths");
        let start = self.pos;
        let data = self.dec.data;
        let value = if start <= data.len() && data.len() - start >= 8 {
            // SAFETY: `start + 8 <= data.len()` was checked just above, so the
            // unaligned 8-byte load stays inside `data`.
            let word = unsafe { core::ptr::read_unaligned(data.as_ptr().add(start).cast::<u64>()) };
            // One load + byte swap + shift replaces a byte loop; `checked_shr`
            // maps the zero-length payload (shift by 64) to 0.
            u64::from_be(word)
                .checked_shr(64 - 8 * size as u32)
                .unwrap_or(0)
        } else {
            if !self.dec.in_range(start, size) {
                return Err(eof());
            }
            self.dec
                .slice(start, size)
                .iter()
                .fold(0_u64, |acc, &b| (acc << 8) | u64::from(b))
        };
        self.pos = if resume == INLINE {
            start + size
        } else {
            resume
        };
        Ok(value)
    }

    /// Reads a UTF-8 string (`&str` fields).
    #[inline]
    pub fn read_str(&mut self) -> Result<&'a str> {
        let (kind, size, resume) = self.header()?;
        if kind != T_UTF8 {
            return Err(mismatch("expected UTF-8 string"));
        }
        self.budget.charge_bytes(size)?;
        utf8(self.payload(size, resume)?)
    }

    /// Reads a byte string (`&[u8]` fields).
    #[inline]
    pub fn read_bytes(&mut self) -> Result<&'a [u8]> {
        let (kind, size, resume) = self.header()?;
        if kind != T_BYTES {
            return Err(mismatch("expected byte array"));
        }
        self.budget.charge_bytes(size)?;
        self.payload(size, resume)
    }

    /// Reads an MMDB uint16/uint32/uint64 (fields of type `u16`/`u32`/`u64`).
    #[inline]
    pub fn read_u64(&mut self) -> Result<u64> {
        let (kind, size, resume) = self.header()?;
        let max = match kind {
            T_U16 => 2,
            T_U32 => 4,
            T_U64 => 8,
            _ => return Err(mismatch("expected numeric value")),
        };
        if size > max {
            return Err(mismatch("integer payload is too large"));
        }
        self.uint_payload(size, resume)
    }

    /// Reads any MMDB unsigned integer, including uint128 (`u128` fields).
    #[inline]
    pub fn read_u128(&mut self) -> Result<u128> {
        let (kind, size, resume) = self.header()?;
        let max = match kind {
            T_U16 => 2,
            T_U32 => 4,
            T_U64 => 8,
            T_U128 => 16,
            _ => return Err(mismatch("expected unsigned integer")),
        };
        if size > max {
            return Err(mismatch("integer payload is too large"));
        }
        if size <= 8 {
            return self.uint_payload(size, resume).map(u128::from);
        }
        let bytes = self.payload(size, resume)?;
        Ok(bytes
            .iter()
            .fold(0_u128, |acc, &b| (acc << 8) | u128::from(b)))
    }

    /// Reads an MMDB int32 (`i32` fields).
    #[inline]
    pub fn read_i32(&mut self) -> Result<i32> {
        let (kind, size, resume) = self.header()?;
        if kind != T_I32 {
            return Err(mismatch("expected int32"));
        }
        if size > 4 {
            return Err(mismatch("int32 is longer than 4 bytes"));
        }
        let raw = self.uint_payload(size, resume)? as u32;
        // Only a full 4-byte payload carries a sign bit; shorter payloads are
        // zero-extended, matching the generic decoder.
        Ok(raw as i32)
    }

    /// Reads an MMDB double or float (`f64` fields).
    #[inline]
    pub fn read_f64(&mut self) -> Result<f64> {
        let (kind, size, resume) = self.header()?;
        match kind {
            T_DOUBLE => {
                if size != 8 {
                    return Err(mismatch("double must contain 8 bytes"));
                }
                let bytes = self.payload(8, resume)?;
                Ok(f64::from_be_bytes(bytes.try_into().map_err(|_| eof())?))
            }
            T_FLOAT => {
                if size != 4 {
                    return Err(mismatch("float must contain 4 bytes"));
                }
                let bytes = self.payload(4, resume)?;
                Ok(f64::from(f32::from_be_bytes(
                    bytes.try_into().map_err(|_| eof())?,
                )))
            }
            _ => Err(mismatch("expected float/double")),
        }
    }

    /// Reads an MMDB float (`f32` fields).
    #[inline]
    pub fn read_f32(&mut self) -> Result<f32> {
        let (kind, size, resume) = self.header()?;
        if kind != T_FLOAT {
            return Err(mismatch("expected float"));
        }
        if size != 4 {
            return Err(mismatch("float must contain 4 bytes"));
        }
        let bytes = self.payload(4, resume)?;
        Ok(f32::from_be_bytes(bytes.try_into().map_err(|_| eof())?))
    }

    /// Reads an MMDB boolean (`bool` fields).
    #[inline]
    pub fn read_bool(&mut self) -> Result<bool> {
        let (kind, size, resume) = self.header()?;
        if kind != T_BOOL {
            return Err(mismatch("expected boolean"));
        }
        if size > 1 {
            return Err(mismatch("boolean size must be zero or one"));
        }
        self.done(resume);
        Ok(size == 1)
    }

    /// Opens the container under the cursor after checking its type.
    #[inline(always)]
    fn enter(&mut self, expected: u8, message: &'static str) -> Result<Container> {
        let (kind, size, resume) = self.header()?;
        if kind != expected {
            return Err(mismatch(message));
        }
        if size > MAX_CONTAINER_ITEMS {
            return Err(Error::ResourceLimit("MMDB container item limit exceeded"));
        }
        if self.depth >= MAX_DEPTH {
            return Err(too_deep());
        }
        self.depth += 1;
        Ok(Container { len: size, resume })
    }

    /// Opens a map; `message` is the type-mismatch error.
    #[inline]
    pub fn enter_map(&mut self, message: &'static str) -> Result<Container> {
        self.enter(T_MAP, message)
    }

    /// Opens an array; `message` is the type-mismatch error.
    #[inline]
    pub fn enter_array(&mut self, message: &'static str) -> Result<Container> {
        self.enter(T_ARRAY, message)
    }

    /// Upper bound for pre-allocating the elements of `container`.
    ///
    /// Every element occupies at least one encoded byte, so a declared length
    /// larger than the remaining data is necessarily bogus. The fixed cap keeps
    /// a hostile length from reserving a huge buffer before the first element
    /// fails to decode.
    #[inline]
    pub fn capacity_hint(&self, container: Container) -> usize {
        let remaining = self.dec.data.len().saturating_sub(self.pos);
        container.len.min(remaining).min(1024)
    }

    /// Closes a container whose entries were all consumed.
    #[inline(always)]
    pub fn leave(&mut self, container: Container) {
        self.depth -= 1;
        if container.resume != INLINE {
            self.pos = container.resume;
        }
    }

    /// Closes a map before reading its last `remaining` entries.
    ///
    /// A map reached through a pointer, or the top-level record itself, is not
    /// followed by anything the caller reads, so nothing needs to be skipped.
    /// Only an inline nested map must be crossed to reach the parent's next
    /// entry.
    #[inline]
    pub fn finish_map(&mut self, container: Container, remaining: usize) -> Result<()> {
        self.depth -= 1;
        if container.resume != INLINE {
            self.pos = container.resume;
            Ok(())
        } else if remaining == 0 || self.depth == 0 {
            Ok(())
        } else {
            self.skip_values(remaining.saturating_mul(2))
        }
    }

    /// Reads a map key as raw bytes.
    ///
    /// The bytes are not UTF-8 validated: generated code only compares them
    /// with field names, and equality with a `&str` literal already proves
    /// validity. Use [`Self::read_key_str`] when the key itself is returned.
    #[inline]
    pub fn read_key(&mut self) -> Result<&'a [u8]> {
        let (kind, size, resume) = self.header()?;
        if kind != T_UTF8 {
            return Err(mismatch("MMDB map key is not UTF-8"));
        }
        self.budget.charge_bytes(size)?;
        self.payload(size, resume)
    }

    /// Reads a validated UTF-8 map key.
    #[inline]
    pub fn read_key_str(&mut self) -> Result<&'a str> {
        utf8(self.read_key()?)
    }

    /// Skips the value under the cursor without decoding it.
    #[inline]
    pub fn skip_value(&mut self) -> Result<()> {
        self.skip_values(1)
    }

    /// Skips `pending` consecutive values.
    ///
    /// Containers add their children to `pending` instead of recursing, and
    /// pointers are stepped over rather than followed, so the loop runs at most
    /// once per crossed byte (bounded again by the value budget) and uses O(1)
    /// stack even for hostile nesting.
    #[inline(never)]
    fn skip_values(&mut self, mut pending: usize) -> Result<()> {
        let mut pos = self.pos;
        while pending != 0 {
            pending -= 1;
            self.budget.charge_value()?;
            let control = self.byte(pos)?;
            pos += 1;
            let mut kind = control >> 5;
            if kind == 1 {
                // Pointer: 1..=4 extra bytes; never followed while skipping.
                pos += 1 + usize::from((control >> 3) & 0x03);
                continue;
            }
            if kind == 0 {
                let ext = self.byte(pos)?;
                pos += 1;
                if ext > 248 {
                    return Err(Error::InvalidDataType(ext));
                }
                kind = ext + 7;
            }
            let (size, extra) = self.dec.decode_size(control & 0x1f, pos)?;
            pos += extra;
            let max_payload = match kind {
                T_MAP | T_ARRAY => {
                    if size > MAX_CONTAINER_ITEMS {
                        return Err(Error::ResourceLimit("MMDB container item limit exceeded"));
                    }
                    let children = if kind == T_MAP { size * 2 } else { size };
                    pending = pending.checked_add(children).ok_or_else(eof)?;
                    continue;
                }
                T_UTF8 | T_BYTES => usize::MAX,
                T_DOUBLE if size == 8 => 8,
                T_FLOAT if size == 4 => 4,
                T_U16 => 2,
                T_U32 | T_I32 => 4,
                T_U64 => 8,
                T_U128 => 16,
                T_BOOL if size <= 1 => {
                    continue;
                }
                T_DOUBLE | T_FLOAT | T_BOOL => {
                    return Err(mismatch("invalid scalar size"));
                }
                other => return Err(Error::InvalidDataType(other)),
            };
            if size > max_payload {
                return Err(mismatch("integer payload is too large"));
            }
            pos += size;
            if pos > self.dec.data.len() {
                return Err(eof());
            }
        }
        if pos > self.dec.data.len() {
            return Err(eof());
        }
        self.pos = pos;
        Ok(())
    }

    /// Materializes the value under the cursor as a generic [`ValueRef`].
    ///
    /// This is the compatibility path for hand-written `MmdbDecode` /
    /// `DecodeField` implementations; it allocates for maps and arrays.
    pub fn read_value(&mut self) -> Result<ValueRef<'a>> {
        let (value, next) = self
            .dec
            .decode_inner(self.pos, self.depth, &mut self.budget)?;
        self.pos = next;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "writer")]
    use crate::{Value, encoder::encode_value};
    #[cfg(feature = "writer")]
    use std::collections::BTreeMap;

    fn decoder(data: &[u8]) -> RawDecoder<'_> {
        RawDecoder::new(data, 0, data.len(), 0)
    }

    #[cfg(feature = "writer")]
    fn encoded(value: Value) -> Vec<u8> {
        let mut data = Vec::new();
        encode_value(&value, &mut data).unwrap();
        data
    }

    #[test]
    fn scalars_inline_and_through_pointers() {
        // 0: "ab"   3: uint32 0x0102   6: pointer -> 0   8: pointer -> 3
        let data = [0x42, b'a', b'b', 0xc2, 0x01, 0x02, 0x20, 0x00, 0x20, 0x03];
        let mut d = decoder(&data);
        assert_eq!(d.read_str().unwrap(), "ab");
        assert_eq!(d.read_u64().unwrap(), 0x0102);
        assert_eq!(d.read_str().unwrap(), "ab");
        assert_eq!(d.pos, 8);
        assert_eq!(d.read_u64().unwrap(), 0x0102);
        assert_eq!(d.pos, 10);
    }

    #[test]
    fn integer_payload_widths_match_generic_decoder() {
        for size in 0..=8_usize {
            // uint64 is an extended type: control `size`, then `9 - 7`.
            let mut data = vec![size as u8, 2];
            data.extend((0..size).map(|i| 0x10 + i as u8));
            let expected = match Decoder::new(&data, 0, data.len()).decode_at(0).unwrap().0 {
                ValueRef::Uint64(v) => v,
                other => panic!("unexpected {other:?}"),
            };
            // Unpadded buffers exercise the byte loop, padded ones the wide load.
            assert_eq!(decoder(&data).read_u64().unwrap(), expected, "size {size}");
            let mut padded = data.clone();
            padded.extend_from_slice(&[0xff; 8]);
            assert_eq!(
                decoder(&padded).read_u64().unwrap(),
                expected,
                "size {size}"
            );
        }
    }

    #[test]
    fn skip_crosses_nested_containers_without_following_pointers() {
        // map{ "k": [1, ptr->99 (never followed), {"x": true}] } then "z"
        let data = [
            0xe1, 0x41, b'k', 0x03, 0x04, 0xa1, 0x01, 0x20, 0x63, 0xe1, 0x41, b'x', 0x01, 0x07,
            0x41, b'z',
        ];
        let mut d = decoder(&data);
        d.skip_value().unwrap();
        assert_eq!(d.read_str().unwrap(), "z");
    }

    #[test]
    fn truncation_and_bad_types_are_errors() {
        assert!(decoder(&[0x42, b'a']).read_str().is_err());
        assert!(decoder(&[0x42, b'a']).skip_value().is_err());
        assert!(decoder(&[0xe2, 0x41]).skip_value().is_err());
        // Extended type 13 (data cache container) is never valid.
        assert!(matches!(
            decoder(&[0x00, 0x06]).skip_value(),
            Err(Error::InvalidDataType(13))
        ));
        assert!(decoder(&[0x41, b'a']).read_u64().is_err());
    }

    #[test]
    fn pointer_cycles_hit_the_depth_limit() {
        let data = [0x20, 0x00];
        assert!(matches!(
            decoder(&data).read_str(),
            Err(Error::ResourceLimit(_))
        ));
    }

    #[test]
    fn early_finished_inline_map_skips_remaining_entries_when_nested() {
        // array[ map{"a": 1, "b": 2}, "tail" ]
        let data = [
            0x02, 0x04, 0xe2, 0x41, b'a', 0xa1, 0x01, 0x41, b'b', 0xa1, 0x02, 0x44, b't', b'a',
            b'i', b'l',
        ];
        let mut d = decoder(&data);
        let array = d.enter_array("expected array").unwrap();
        assert_eq!(array.len(), 2);
        let map = d.enter_map("expected map").unwrap();
        assert_eq!(d.read_key().unwrap(), b"a");
        assert_eq!(d.read_u64().unwrap(), 1);
        d.finish_map(map, 1).unwrap();
        assert_eq!(d.read_str().unwrap(), "tail");
        d.leave(array);
    }

    #[cfg(feature = "writer")]
    #[test]
    fn typed_scalars_match_the_generic_decoder() {
        let bytes = encoded(Value::Bytes(vec![0, 127, 255]));
        assert_eq!(decoder(&bytes).read_bytes().unwrap(), &[0, 127, 255]);

        for (value, expected) in [
            (Value::Uint16(50), 50_u64),
            (Value::Uint32(65_000), 65_000),
            (Value::Uint64(1 << 40), 1 << 40),
        ] {
            assert_eq!(decoder(&encoded(value)).read_u64().unwrap(), expected);
        }
        for (value, expected) in [
            (Value::Uint16(50), 50_u128),
            (Value::Uint64(1 << 40), 1 << 40),
            (Value::Uint128(1 << 100), 1 << 100),
        ] {
            assert_eq!(decoder(&encoded(value)).read_u128().unwrap(), expected);
        }
        for n in [-123_i32, 0, 123] {
            assert_eq!(decoder(&encoded(Value::Int32(n))).read_i32().unwrap(), n);
        }
        assert_eq!(
            decoder(&encoded(Value::Double(1.25))).read_f64().unwrap(),
            1.25
        );
        assert_eq!(
            decoder(&encoded(Value::Float(1.25))).read_f64().unwrap(),
            1.25
        );
        assert_eq!(
            decoder(&encoded(Value::Float(1.25))).read_f32().unwrap(),
            1.25
        );
        for value in [false, true] {
            assert_eq!(
                decoder(&encoded(Value::Bool(value))).read_bool().unwrap(),
                value
            );
        }
    }

    #[cfg(feature = "writer")]
    #[test]
    fn containers_keys_and_materialization_follow_cursor_rules() {
        let map = Value::Map(BTreeMap::from([("name".into(), Value::Utf8("ok".into()))]));
        let bytes = encoded(map.clone());
        let mut d = decoder(&bytes);
        let container = d.enter_map("expected map").unwrap();
        assert_eq!(container.len(), 1);
        assert!(!container.is_empty());
        assert_eq!(d.capacity_hint(container), 1);
        assert_eq!(d.read_key_str().unwrap(), "name");
        assert_eq!(d.read_str().unwrap(), "ok");
        d.leave(container);
        assert_eq!(decoder(&bytes).read_value().unwrap().to_owned_value(), map);

        let bytes = encoded(Value::Array(vec![]));
        let mut d = decoder(&bytes);
        let container = d.enter_array("expected array").unwrap();
        assert!(container.is_empty());
        assert_eq!(d.capacity_hint(container), 0);
        d.finish_map(container, 0).unwrap();

        // A pointer to an array must restore the cursor after the pointer.
        let data = [0x20, 0x04, 0x41, b'z', 0x01, 0x04, 0xa1, 1];
        let mut d = decoder(&data);
        let container = d.enter_array("expected array").unwrap();
        assert_eq!(d.read_u64().unwrap(), 1);
        d.leave(container);
        assert_eq!(d.read_str().unwrap(), "z");

        let mut d = decoder(&data);
        let container = d.enter_array("expected array").unwrap();
        d.finish_map(container, 1).unwrap();
        assert_eq!(d.read_str().unwrap(), "z");
    }

    #[cfg(feature = "writer")]
    #[test]
    fn typed_reads_reject_mismatches_and_invalid_widths() {
        let text = encoded(Value::Utf8("x".into()));
        assert!(decoder(&text).read_bytes().is_err());
        assert!(decoder(&text).read_u128().is_err());
        assert!(decoder(&text).read_i32().is_err());
        assert!(decoder(&text).read_f64().is_err());
        assert!(decoder(&text).read_f32().is_err());
        assert!(decoder(&text).read_bool().is_err());
        assert!(decoder(&text).enter_map("expected map").is_err());
        assert!(decoder(&text).enter_array("expected array").is_err());
        assert!(decoder(&text).read_key().is_ok());
        assert!(decoder(&[0x41, 0xff]).read_key_str().is_err());
        assert!(decoder(&[]).read_str().is_err());
        assert!(decoder(&[0x00, 249]).read_str().is_err());

        // Each malformed payload declares more bytes than its MMDB type permits.
        for bytes in [
            vec![0xa3, 0, 0, 1],       // uint16, 3 bytes
            vec![0xc5, 0, 0, 0, 0, 1], // uint32, 5 bytes
            vec![0x09, 2],             // uint64, 9 bytes
        ] {
            assert!(decoder(&bytes).read_u64().is_err());
            assert!(decoder(&bytes).read_u128().is_err());
        }
        assert!(decoder(&[0x11, 3]).read_u128().is_err()); // uint128, 17 bytes
        assert!(decoder(&[0x05, 1]).read_i32().is_err()); // int32, 5 bytes
        assert!(decoder(&[0x07, 8]).read_f64().is_err()); // double, 7 bytes
        assert!(decoder(&[0x05, 8]).read_f64().is_err()); // float, 5 bytes
        assert!(decoder(&[0x03, 8]).read_f32().is_err()); // float, 3 bytes
        assert!(decoder(&[0x02, 7]).read_bool().is_err()); // boolean, size 2
        assert!(decoder(&[0x43, b'a']).read_str().is_err()); // truncated string
    }

    #[cfg(feature = "writer")]
    #[test]
    fn skip_validates_types_widths_and_boundaries() {
        for value in [
            Value::Utf8("ok".into()),
            Value::Bytes(vec![1, 2]),
            Value::Double(1.0),
            Value::Float(1.0),
            Value::Uint16(1),
            Value::Uint32(1),
            Value::Int32(-1),
            Value::Uint64(1),
            Value::Uint128(1),
            Value::Bool(false),
            Value::Bool(true),
            Value::Array(vec![Value::Uint16(1)]),
            Value::Map(BTreeMap::from([("k".into(), Value::Bool(true))])),
        ] {
            let bytes = encoded(value);
            let mut d = decoder(&bytes);
            d.skip_value().unwrap();
            assert_eq!(d.pos, bytes.len());
        }
        for bytes in [
            vec![0x68],       // double with wrong size
            vec![0x03, 8],    // float with wrong size
            vec![0x02, 7],    // boolean with wrong size
            vec![0xa3],       // uint16 with wrong size
            vec![0x00, 249],  // reserved extended type
            vec![0x20],       // truncated pointer
            vec![0x42, b'a'], // truncated payload
        ] {
            assert!(decoder(&bytes).skip_value().is_err(), "{bytes:?}");
        }
    }

    #[cfg(feature = "writer")]
    #[test]
    fn pointer_booleans_restore_cursor_and_malformed_keys_fail() {
        // The pointed-to boolean has no payload; the next inline value must
        // still be read from immediately after the pointer.
        let data = [0x20, 0x04, 0x41, b'z', 0x01, 0x07];
        let mut d = decoder(&data);
        assert!(d.read_bool().unwrap());
        assert_eq!(d.read_str().unwrap(), "z");

        assert!(decoder(&[0xa1, 1]).read_key().is_err());
        assert!(decoder(&[0x42, b'a']).read_value().is_err());
        assert!(decoder(&[0x20]).read_value().is_err());
        assert!(decoder(&[0x20, 0x00]).skip_value().is_ok());
        assert!(decoder(&[0x2f]).skip_value().is_err());

        let bytes = encoded(Value::Array(vec![]));
        let mut d = decoder(&bytes);
        d.depth = MAX_DEPTH;
        assert!(matches!(
            d.enter_array("expected array"),
            Err(Error::ResourceLimit(_))
        ));

        // A pointer chain uses the original pointer's resume position.
        let chain = [0x20, 0x04, 0x41, b'z', 0x20, 0x06, 0x41, b'a'];
        let mut d = decoder(&chain);
        assert_eq!(d.read_str().unwrap(), "a");
        assert_eq!(d.read_str().unwrap(), "z");
        assert!(matches!(
            decoder(&[0x20, 0x63]).read_str(),
            Err(Error::UnexpectedEof)
        ));
    }
}
