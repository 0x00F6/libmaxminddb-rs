//! MMDB data-section decoder.
//!
//! Compiled in every feature configuration: `#[derive(MmdbDecode)]` expands to
//! code that names [`RawDecoder`], and a derive cannot see which features of
//! this crate are enabled. Without `reader`, nothing calls into the module.
#![cfg_attr(not(feature = "reader"), allow(dead_code))]

mod ascii;
mod raw;

pub use raw::{Container, RawDecoder};

use crate::{Error, Result, ValueRef};

const MAX_DEPTH: usize = 512;
const MAX_CONTAINER_ITEMS: usize = 16_843_036;
const MAX_DECODED_VALUES: usize = 1_000_000;
const MAX_EXPANDED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
struct DecodeBudget {
    values: usize,
    expanded_bytes: usize,
}

impl DecodeBudget {
    #[inline(always)]
    fn charge_value(&mut self) -> Result<()> {
        // `values` is checked at one million on every increment, so it cannot approach
        // usize::MAX before this addition.  Avoiding saturating arithmetic keeps the
        // success path to one add + one highly predictable comparison.
        self.values += 1;
        if self.values > MAX_DECODED_VALUES {
            return Err(Error::ResourceLimit("MMDB decoded-value budget exceeded"));
        }
        Ok(())
    }

    #[inline(always)]
    fn charge_bytes(&mut self, bytes: usize) -> Result<()> {
        // MMDB's encoded size tops out far below usize::MAX and this accumulator is
        // rejected after 64 MiB, therefore this addition cannot overflow first.
        self.expanded_bytes += bytes;
        if self.expanded_bytes > MAX_EXPANDED_BYTES {
            return Err(Error::ResourceLimit("MMDB expanded-byte budget exceeded"));
        }
        Ok(())
    }
}

/// Zero-copy decoder for one MMDB data section.
///
/// `data` is pre-sliced to `[..limit]` so every `get`/slice bounds check
/// against `data.len()` is equivalent to the original limit check without
/// an extra comparison.
pub(crate) struct Decoder<'a> {
    data: &'a [u8],
    pointer_base: usize,
}

/// Leaf-scalar fast path for container elements (map values, array items).
///
/// Expands to a `Result<Option<(ValueRef<'a>, usize)>>`: `Ok(None)` when the
/// control byte does not name a leaf scalar type (containers, extended type,
/// pointers, invalid types fall through to the recursive parser), otherwise the
/// value and following cursor. It must reproduce the general path exactly: one
/// `charge_value`, `decode_size`, and `decode_scalar`.
///
/// Written as a macro so the body is guaranteed to be inlined inside
/// `decode_container`'s loops; a separate `#[inline(always)]` method was emitted
/// by LLVM as a real per-element call inside that large function and measured
/// slower than the recursion it replaces. Container bases from depth `depth`
/// call this for elements at `depth + 1`, so the enclosing path has already
/// proven the depth bound and the fast path skips its own entry depth check.
macro_rules! leaf_decode {
    ($self:expr, $control:expr, $pos:expr, $budget:expr) => {{
        let control: u8 = $control;
        let data_type = control >> 5;
        match data_type {
            2 | 3 | 4 | 5 | 6 | 8 | 9 | 10 | 14 | 15 => {
                $budget.charge_value()?;
                let (size, size_bytes) = $self.decode_size(control & 0x1f, $pos)?;
                let (value, next) =
                    $self.decode_scalar(data_type, size, $pos + size_bytes, $budget)?;
                Ok(Some((value, next)))
            }
            1 => {
                if let Ok((pointer, consumed)) = $self.decode_pointer(control, $pos) {
                    if let Some(target) = $self.pointer_base.checked_add(pointer) {
                        if let Some(&target_control) = $self.data.get(target) {
                            let target_type = target_control >> 5;
                            match target_type {
                                2 | 3 | 4 | 5 | 6 | 8 | 9 | 10 | 14 | 15 => {
                                    $budget.charge_value()?;
                                    $budget.charge_value()?;
                                    let (size, size_bytes) =
                                        $self.decode_size(target_control & 0x1f, target + 1)?;
                                    let (value, _) = $self.decode_scalar(
                                        target_type,
                                        size,
                                        target + 1 + size_bytes,
                                        $budget,
                                    )?;
                                    let next = $pos + consumed;
                                    Ok(Some((value, next)))
                                }
                                _ => Ok(None),
                            }
                        } else {
                            Ok(None)
                        }
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
            _ => Ok::<Option<(ValueRef<'a>, usize)>, Error>(None),
        }
    }};
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(data: &'a [u8], pointer_base: usize, limit: usize) -> Self {
        Self {
            // limit is always <= data.len() in current call-sites; clamp defensively.
            data: &data[..limit.min(data.len())],
            pointer_base,
        }
    }

    pub(crate) fn decode_at(&self, offset: usize) -> Result<(ValueRef<'a>, usize)> {
        let mut budget = DecodeBudget::default();
        self.decode_inner(offset, 0, &mut budget)
    }

    // `ok_or_else` keeps `Error` construction off the success path; clippy's
    // `unnecessary_lazy_evaluations` does not account for the eager cost here.
    #[allow(clippy::unnecessary_lazy_evaluations)]
    fn decode_inner(
        &self,
        mut offset: usize,
        depth: usize,
        budget: &mut DecodeBudget,
    ) -> Result<(ValueRef<'a>, usize)> {
        if depth > MAX_DEPTH {
            return Err(Error::ResourceLimit("maximum MMDB nesting depth exceeded"));
        }
        budget.charge_value()?;
        // One bounds branch per recursion; this is also the net for arbitrary
        // pointer targets that resolve inside `data` (pointer_base + pointer).
        if !self.in_range(offset, 1) {
            return Err(Error::UnexpectedEof);
        }
        let control = unsafe { *self.data.get_unchecked(offset) };
        offset += 1;
        let mut data_type = control >> 5;

        if data_type == 1 {
            let (pointer, consumed) = self.decode_pointer(control, offset)?;
            let target = self
                .pointer_base
                .checked_add(pointer)
                .ok_or_else(|| Error::InvalidOffset(pointer))?;
            let (value, _) = self.decode_inner(target, depth + 1, budget)?;
            return Ok((value, offset + consumed));
        }

        if data_type == 0 {
            if !self.in_range(offset, 1) {
                return Err(Error::UnexpectedEof);
            }
            let ext = unsafe { *self.data.get_unchecked(offset) };
            offset += 1;
            // Bytes 249..=255 are reserved type-descriptor continuation; adding 7
            // would overflow u8, so reject them as an unsupported data type.
            if ext > 248 {
                return Err(Error::InvalidDataType(ext));
            }
            data_type = ext + 7;
        }

        let (size, size_bytes) = self.decode_size(control & 0x1f, offset)?;
        offset += size_bytes;
        if size > MAX_CONTAINER_ITEMS && matches!(data_type, 7 | 11) {
            return Err(Error::ResourceLimit("MMDB container item limit exceeded"));
        }

        let (value, next) = match data_type {
            // Containers decode in `decode_container` (never inlined), which keeps
            // `decode_inner` a small, icache-friendly body for the dominant root-
            // scalar, pointer and recursion cases. The deep/cyclic nesting probes
            // run on an explicitly large-stack test thread (see `probe_deep`).
            7 => self.decode_container(true, size, offset, depth, budget),
            11 => self.decode_container(false, size, offset, depth, budget),
            _ => self.decode_scalar(data_type, size, offset, budget),
        }?;
        Ok((value, next))
    }

    // Map (is_map == true) and array element decoding, including the inlined leaf
    // fast path. `#[inline(never)]` keeps the bulky
    // container loops (and the macro-expanded leaf body) out of `decode_inner`'s
    // icache footprint. Callers guarantee `depth + 1 <= MAX_DEPTH` before any
    // leaf, so the fast path skips its own entry depth check; `decode_inner` still
    // guards the boundary case.
    #[inline(never)]
    #[allow(clippy::unnecessary_lazy_evaluations)]
    fn decode_container(
        &self,
        is_map: bool,
        size: usize,
        offset: usize,
        depth: usize,
        budget: &mut DecodeBudget,
    ) -> Result<(ValueRef<'a>, usize)> {
        if is_map {
            let mut entries = Vec::with_capacity(size.min(256));
            let mut cursor = offset;
            let leaf_depth_ok = depth < MAX_DEPTH;

            // Software prefetch: hint CPU to load next cache line for sequential access
            // This helps when iterating through container elements
            #[cfg(all(target_arch = "x86_64", feature = "simd"))]
            {
                if size > 2 && cursor < self.data.len() {
                    // SAFETY: wrapping_add may form an out-of-bounds pointer without
                    // asserting Rust's in-allocation `add` invariant. PREFETCH
                    // is a non-faulting hint and never dereferences that pointer.
                    unsafe {
                        core::arch::x86_64::_mm_prefetch(
                            self.data
                                .as_ptr()
                                .wrapping_add(cursor.saturating_add(64))
                                .cast(),
                            core::arch::x86_64::_MM_HINT_T0,
                        );
                    }
                }
            }
            for _ in 0..size {
                let (key, next) = self.decode_key(cursor, depth + 1, budget)?;
                cursor = next;
                if leaf_depth_ok
                    && self.in_range(cursor, 1)
                    && let &control = unsafe {self.data.get_unchecked(cursor)}
                    // SAFETY of structure: leaf_decode! performs its own budget
                    // charges and bounds checks via the shared size/scalar
                    // decoders; Ok(None) leaves cursor untouched for the
                    // recursive fallback.
                    && let Some((value, next)) =
                        leaf_decode!(self, control, cursor + 1, budget)?
                {
                    cursor = next;
                    entries.push((key, value));
                    continue;
                }
                let (value, next) = self.decode_inner(cursor, depth + 1, budget)?;
                cursor = next;
                entries.push((key, value));
            }
            Ok((ValueRef::Map(entries), cursor))
        } else {
            let mut values = Vec::with_capacity(size.min(256));
            let mut cursor = offset;
            let leaf_depth_ok = depth < MAX_DEPTH;

            // Software prefetch for arrays
            #[cfg(all(target_arch = "x86_64", feature = "simd"))]
            {
                if size > 2 && cursor < self.data.len() {
                    // SAFETY: wrapping_add may form an out-of-bounds pointer without
                    // asserting Rust's in-allocation `add` invariant. PREFETCH
                    // is a non-faulting hint and never dereferences that pointer.
                    unsafe {
                        core::arch::x86_64::_mm_prefetch(
                            self.data
                                .as_ptr()
                                .wrapping_add(cursor.saturating_add(64))
                                .cast(),
                            core::arch::x86_64::_MM_HINT_T0,
                        );
                    }
                }
            }
            for _ in 0..size {
                if leaf_depth_ok
                    && self.in_range(cursor, 1)
                    && let &control = unsafe {self.data.get_unchecked(cursor)}
                    // SAFETY of structure: leaf_decode! performs its own budget
                    // charges and bounds checks via the shared size/scalar
                    // decoders; Ok(None) leaves cursor untouched for the
                    // recursive fallback.
                    && let Some((value, next)) =
                        leaf_decode!(self, control, cursor + 1, budget)?
                {
                    cursor = next;
                    values.push(value);
                    continue;
                }
                let (value, next) = self.decode_inner(cursor, depth + 1, budget)?;
                values.push(value);
                cursor = next;
            }
            Ok((ValueRef::Array(values), cursor))
        }
    }

    // Keys dominate small maps. Decode direct strings without constructing a ValueRef
    // or entering the general type switch. Pointer/non-string keys retain the ordinary
    // parser and its exact validation/budget behavior.
    #[allow(clippy::collapsible_if)]
    #[inline(always)]
    fn decode_key(
        &self,
        offset: usize,
        depth: usize,
        budget: &mut DecodeBudget,
    ) -> Result<(&'a str, usize)> {
        if depth > MAX_DEPTH {
            return Err(Error::ResourceLimit("maximum MMDB nesting depth exceeded"));
        }
        if let Some(&control) = self.data.get(offset) {
            let data_type = control >> 5;
            if data_type == 2 {
                budget.charge_value()?;
                let offset = offset + 1;
                let (size, extra) = self.decode_size(control & 0x1f, offset)?;
                let offset = offset + extra;
                budget.charge_bytes(size)?;
                if !self.in_range(offset, size) {
                    return Err(Error::UnexpectedEof);
                }
                return Ok((utf8(self.slice(offset, size))?, offset + size));
            } else if data_type == 1
                && let Ok((pointer, consumed)) = self.decode_pointer(control, offset + 1)
                && let Some(target) = self.pointer_base.checked_add(pointer)
                && let Some(&target_control) = self.data.get(target)
                && target_control >> 5 == 2
            {
                budget.charge_value()?;
                if depth + 1 > MAX_DEPTH {
                    return Err(Error::ResourceLimit("maximum MMDB nesting depth exceeded"));
                }
                budget.charge_value()?;
                let t_offset = target + 1;
                let (size, extra) = self.decode_size(target_control & 0x1f, t_offset)?;
                let t_offset = t_offset + extra;
                budget.charge_bytes(size)?;
                if !self.in_range(t_offset, size) {
                    return Err(Error::UnexpectedEof);
                }
                return Ok((utf8(self.slice(t_offset, size))?, offset + 1 + consumed));
            }
        }
        let (key, next) = self.decode_inner(offset, depth, budget)?;
        match key {
            ValueRef::Utf8(key) => Ok((key, next)),
            _ => Err(Error::DecodingError("MMDB map key is not UTF-8".into())),
        }
    }

    // Keep non-recursive scalar temporaries out of each recursive container frame.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn decode_scalar(
        &self,
        data_type: u8,
        size: usize,
        offset: usize,
        budget: &mut DecodeBudget,
    ) -> Result<(ValueRef<'a>, usize)> {
        match data_type {
            2 => {
                budget.charge_bytes(size)?;
                if !self.in_range(offset, size) {
                    return Err(Error::UnexpectedEof);
                }
                let bytes = self.slice(offset, size);
                let text = utf8(bytes)?;
                Ok((ValueRef::Utf8(text), offset + size))
            }
            3 => {
                if size != 8 {
                    return Err(Error::DecodingError("double must contain 8 bytes".into()));
                }
                if !self.in_range(offset, 8) {
                    return Err(Error::UnexpectedEof);
                }
                let bytes: [u8; 8] = self.slice(offset, 8).try_into().expect("length checked");
                Ok((ValueRef::Double(f64::from_be_bytes(bytes)), offset + 8))
            }
            4 => {
                budget.charge_bytes(size)?;
                if !self.in_range(offset, size) {
                    return Err(Error::UnexpectedEof);
                }
                Ok((ValueRef::Bytes(self.slice(offset, size)), offset + size))
            }
            5 => {
                let v = self.read_uint(offset, size, 2)? as u16;
                Ok((ValueRef::Uint16(v), offset + size))
            }
            6 => {
                let v = self.read_uint(offset, size, 4)? as u32;
                Ok((ValueRef::Uint32(v), offset + size))
            }
            8 => {
                if size > 4 {
                    return Err(Error::DecodingError("int32 is longer than 4 bytes".into()));
                }
                let raw = self.read_uint(offset, size, 4)? as u32;
                let value = if size == 4 {
                    i32::from_be_bytes(raw.to_be_bytes())
                } else {
                    raw as i32
                };
                Ok((ValueRef::Int32(value), offset + size))
            }
            9 => {
                let v = self.read_uint(offset, size, 8)? as u64;
                Ok((ValueRef::Uint64(v), offset + size))
            }
            10 => {
                let v = self.read_uint(offset, size, 16)?;
                Ok((ValueRef::Uint128(v), offset + size))
            }
            13 => Err(Error::InvalidDataType(13)),
            14 => {
                if size > 1 {
                    return Err(Error::DecodingError(
                        "boolean size must be zero or one".into(),
                    ));
                }
                Ok((ValueRef::Bool(size == 1), offset))
            }
            15 => {
                if size != 4 {
                    return Err(Error::DecodingError("float must contain 4 bytes".into()));
                }
                if !self.in_range(offset, 4) {
                    return Err(Error::UnexpectedEof);
                }
                let bytes: [u8; 4] = self.slice(offset, 4).try_into().expect("length checked");
                Ok((ValueRef::Float(f32::from_be_bytes(bytes)), offset + 4))
            }
            other => Err(Error::InvalidDataType(other)),
        }
    }

    /// Decodes the size from a control byte and following bytes.
    ///
    /// Sizes 0-28 are encoded directly in the control byte (fast path, ~95% of cases).
    /// Sizes 29-31 read 1-3 additional bytes (inlined to avoid call overhead).
    #[inline(always)]
    fn decode_size(&self, size: u8, offset: usize) -> Result<(usize, usize)> {
        if size <= 28 {
            // Fast path: sizes 0-28 encoded directly in the 5 low bits
            Ok((usize::from(size), 0))
        } else {
            // Inline sizes 29-31 to avoid function call overhead.
            // Benchmark shows ~40% improvement in lookup hot paths.
            match size {
                29 => {
                    // 29 + 1 byte: encodes sizes 29-284
                    if !self.in_range(offset, 1) {
                        return Err(Error::UnexpectedEof);
                    }
                    // SAFETY: bounds checked above
                    Ok((
                        29 + usize::from(unsafe { *self.data.get_unchecked(offset) }),
                        1,
                    ))
                }
                30 => {
                    // 285 + 2 bytes (big-endian u16): encodes sizes 285-65820
                    if !self.in_range(offset, 2) {
                        return Err(Error::UnexpectedEof);
                    }
                    let bytes: [u8; 2] = self.slice(offset, 2).try_into().expect("length checked");
                    Ok((285 + usize::from(u16::from_be_bytes(bytes)), 2))
                }
                31 => {
                    // 65821 + 3 bytes (big-endian u24): encodes sizes 65821-16843036
                    if !self.in_range(offset, 3) {
                        return Err(Error::UnexpectedEof);
                    }
                    let b = self.slice(offset, 3);
                    // SAFETY: bounds checked above, slice is valid
                    let n = (usize::from(unsafe { *b.get_unchecked(0) }) << 16)
                        | (usize::from(b[1]) << 8)
                        | usize::from(b[2]);
                    Ok((65_821 + n, 3))
                }
                _ => unreachable!(),
            }
        }
    }

    /// Decode a pointer value from the control byte and following bytes.
    ///
    /// # MMDB Pointer Encoding
    /// - Selector 0: 1 extra byte, 11-bit pointer, base offset 0
    /// - Selector 1: 2 extra bytes, 20-bit pointer, base offset 2048
    /// - Selector 2: 3 extra bytes, 29-bit pointer, base offset 526336
    /// - Selector 3: 4 extra bytes, 32-bit pointer, base offset 0
    ///
    /// # Optimizations
    /// - Uses direct pointer reads instead of slice + get_unchecked
    /// - Common selector values (0, 1) are optimized
    #[allow(clippy::unnecessary_lazy_evaluations)]
    #[inline(always)]
    fn decode_pointer(&self, control: u8, offset: usize) -> Result<(usize, usize)> {
        let selector = (control >> 3) & 0x03;
        let high = usize::from(control & 0x07);

        // SAFETY: All pointer accesses below are guarded by bounds checks
        match selector {
            0 => {
                if !self.in_range(offset, 1) {
                    return Err(Error::UnexpectedEof);
                }
                let low = unsafe { *self.data.get_unchecked(offset) };
                Ok(((high << 8) | usize::from(low), 1))
            }
            1 => {
                if !self.in_range(offset, 2) {
                    return Err(Error::UnexpectedEof);
                }
                let b = self.slice(offset, 2);
                let byte0 = unsafe { *b.get_unchecked(0) };
                let byte1 = unsafe { *b.get_unchecked(1) };
                Ok((
                    ((high << 16) | (usize::from(byte0) << 8) | usize::from(byte1)) + 2_048,
                    2,
                ))
            }
            2 => {
                if !self.in_range(offset, 3) {
                    return Err(Error::UnexpectedEof);
                }
                let b = self.slice(offset, 3);
                let byte0 = unsafe { *b.get_unchecked(0) };
                let byte1 = unsafe { *b.get_unchecked(1) };
                let byte2 = unsafe { *b.get_unchecked(2) };
                let raw = (high << 24)
                    | (usize::from(byte0) << 16)
                    | (usize::from(byte1) << 8)
                    | usize::from(byte2);
                Ok((raw + 526_336, 3))
            }
            3 => {
                if !self.in_range(offset, 4) {
                    return Err(Error::UnexpectedEof);
                }
                let bytes = self.slice(offset, 4);
                Ok((u32::from_be_bytes(bytes.try_into().unwrap()) as usize, 4))
            }
            _ => unreachable!(),
        }
    }

    #[allow(clippy::unnecessary_lazy_evaluations)]
    #[inline]
    fn slice(&self, offset: usize, len: usize) -> &'a [u8] {
        // `offset + len` cannot overflow: `offset < self.data.len() <= isize::MAX`
        // (slice lengths are bounded by `isize::MAX`) and `len` is capped far below
        // `usize::MAX` by `decode_size`, so the sum stays below `usize::MAX` too.
        // A single bounds compare replaces the previous `checked_add` + `get(range)`.
        let end = offset + len;

        // SAFETY: every caller proves `end <= data.len()` (and `offset <= end`) via
        // `in_range(offset, len)` before reading, so `[offset..end]` is exclusively
        // within `self.data` (offsets are non-negative by construction).
        unsafe { self.data.get_unchecked(offset..end) }
    }

    // Cheap inlined bounds proof used by every payload read. `slice` stays an
    // unchecked `&[u8]` so the hot reads never funnel through `Result<&[u8], _>`;
    // this branch-guarded form costs ~5% over the pre-hardening baseline.
    #[inline(always)]
    fn in_range(&self, offset: usize, len: usize) -> bool {
        // `offset + len` cannot overflow (same argument as `slice`), and one
        // comparison computes the whole `[offset, offset + len)` validity.
        offset <= self.data.len() && len <= self.data.len() - offset
    }

    /// Read a big-endian unsigned integer of the given byte length.
    ///
    /// # Optimizations
    /// - Always inlined in release builds to eliminate call overhead
    /// - Common sizes (1, 2, 4, 8 bytes) use unaligned pointer reads
    /// - Less common sizes (3, 5-7, 9-15) use byte-at-a-time construction
    /// - Bounds are validated once before any pointer access
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn read_uint(&self, offset: usize, len: usize, max: usize) -> Result<u128> {
        if len > max {
            return Err(Error::DecodingError("integer payload is too large".into()));
        }
        if !self.in_range(offset, len) {
            return Err(Error::UnexpectedEof);
        }

        // SAFETY: bounds were validated above, so all pointer accesses below are safe.
        // We use unaligned reads which are valid for all integer types on all platforms.
        unsafe {
            let ptr = self.data.as_ptr().add(offset);
            match len {
                0 => Ok(0),
                1 => Ok(u128::from(*ptr)),
                2 => Ok(u128::from(u16::from_be(core::ptr::read_unaligned(
                    ptr.cast::<u16>(),
                )))),
                3 => {
                    let a =
                        u128::from(u16::from_be(core::ptr::read_unaligned(ptr.cast::<u16>()))) << 8;
                    let b = u128::from(*ptr.add(2));
                    Ok(a | b)
                }
                4 => Ok(u128::from(u32::from_be(core::ptr::read_unaligned(
                    ptr.cast::<u32>(),
                )))),
                5 => {
                    let a =
                        u128::from(u32::from_be(core::ptr::read_unaligned(ptr.cast::<u32>()))) << 8;
                    let b = u128::from(*ptr.add(4));
                    Ok(a | b)
                }
                6 => {
                    let a = u128::from(u32::from_be(core::ptr::read_unaligned(ptr.cast::<u32>())))
                        << 16;
                    let b = u128::from(u16::from_be(core::ptr::read_unaligned(
                        ptr.add(4).cast::<u16>(),
                    )));
                    Ok(a | b)
                }
                7 => {
                    let a = u128::from(u32::from_be(core::ptr::read_unaligned(ptr.cast::<u32>())))
                        << 24;
                    let b = u128::from(u16::from_be(core::ptr::read_unaligned(
                        ptr.add(4).cast::<u16>(),
                    ))) << 8;
                    let c = u128::from(*ptr.add(6));
                    Ok(a | b | c)
                }
                8 => Ok(u128::from(u64::from_be(core::ptr::read_unaligned(
                    ptr.cast::<u64>(),
                )))),
                9..=16 => {
                    // For 9-16 bytes, use byte-at-a-time construction
                    // This avoids complex pointer arithmetic for uncommon sizes
                    let mut out = 0_u128;
                    for i in 0..len {
                        out = (out << 8) | u128::from(*ptr.add(i));
                    }
                    Ok(out)
                }
                _ => {
                    let mut out = 0_u128;
                    for i in 0..len {
                        out = (out << 8) | u128::from(*ptr.add(i));
                    }
                    Ok(out)
                }
            }
        }
    }
}

/// Decodes a UTF-8 string with an ASCII fast path.
///
/// Most real-world MMDB string payloads (keys and short values) are pure
/// ASCII. A scalar word mask or explicit SIMD proves ASCII validity; only when a
/// non-ASCII byte is found does the decoder fall back to the full
/// [`std::str::from_utf8`] validator, so untrusted multi-byte sequences are
/// never skipped.
///
/// # Optimizations
/// - Empty strings are fast-pathed (common case for empty map keys or values)
/// - ASCII check is inlined and uses SIMD when available
/// - Non-ASCII fallback is cold and rarely taken
#[inline]
fn utf8(bytes: &[u8]) -> Result<&str> {
    // Fast path for empty strings
    if bytes.is_empty() {
        return Ok("");
    }

    if ascii::is_ascii(bytes) {
        // SAFETY: is_ascii proves every byte has its high bit clear, which is
        // a subset of valid UTF-8 (plain ASCII). The unchecked construction is
        // therefore equivalent to a successful validation and adds no unsafe
        // acceptance of non-ASCII or malformed sequences.
        Ok(unsafe { std::str::from_utf8_unchecked(bytes) })
    } else {
        utf8_validated(bytes)
    }
}

/// Out-of-line full UTF-8 validation. Kept cold so the ASCII fast path in
/// [`utf8`] stays small enough to inline into scalar/key decoding.
#[cold]
#[inline(never)]
fn utf8_validated(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| Error::DecodingError("invalid UTF-8 string".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(data: &[u8], start: usize) {
        let decoder = Decoder::new(data, 0, data.len());
        let _ = decoder.decode_at(start);
    }

    // The MAX_DEPTH-limit probes recurse up to 513 frames; a debug `decode_inner`
    // frame is a few KB (the leaf fast path is deliberately kept as a separate
    // non-inlined call in debug), so the boundary probe needs more than the
    // libtest default thread stack. Spawn it on a dedicated thread.
    fn probe_deep(data: &[u8], start: usize) {
        let owned = data.to_vec();
        std::thread::Builder::new()
            .name("decoder-depth".into())
            .stack_size(64 * 1024 * 1024)
            .spawn(move || probe(&owned, start))
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn pointers_containers_cycles_and_truncations_are_bounded() {
        // Root map with a pointer key, a pointer value, array, empty map and empty array.
        let data = [
            0x41, b'k', 0x43, b'v', b'a', b'l', 0xe1, 0x20, 0, 3, 4, 0x20, 2, 0xe0, 0, 4,
        ];
        probe(&data, 6);
        for end in 0..data.len() {
            probe(&data[..end], 6);
        }
        let mut nested = [1, 4].repeat(MAX_DEPTH);
        nested.push(0x40);
        probe_deep(&nested, 0);
        nested.splice(0..0, [1, 4]);
        probe_deep(&nested, 0);
        // Truncations include self-referencing pointer cycles (`[0xe1, 0x20, 0]`
        // at offset 0, and its 0xa0 variant) that recurse ~MAX_DEPTH frames to hit
        // the depth limit, so they get the deep-stack thread as well.
        for data in [
            &[0x20, 0][..],
            &[0xe1, 0x20, 0][..],
            &[0xe1, 0xa0, 0x40][..],
            &[0x5f, 255, 255, 255][..],
            &[0, 255][..],
        ] {
            probe_deep(data, 0);
        }
    }

    #[test]
    fn deterministic_corruption_replay_does_not_panic() {
        let mut state = 0xcafe_f00d_u64;
        for len in 0..128 {
            for _ in 0..64 {
                let mut data = vec![0; len];
                for byte in &mut data {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    *byte = state as u8;
                }
                probe(&data, 0);
            }
        }
    }

    #[test]
    fn decoding_keeps_depth_value_and_byte_budgets() {
        // A backward-pointer DAG doubles the previous array each level.
        let mut dag = vec![0x40];
        let mut previous = 0;
        for _ in 0..20 {
            let offset = dag.len();
            dag.extend_from_slice(&[2, 4, 0x20, previous as u8, 0x20, previous as u8]);
            previous = offset;
        }
        let decoder = Decoder::new(&dag, 0, dag.len());
        assert!(matches!(
            decoder.decode_at(previous),
            Err(Error::ResourceLimit(_))
        ));
        let mut budget = DecodeBudget {
            values: MAX_DECODED_VALUES,
            expanded_bytes: 0,
        };
        assert!(decoder.decode_inner(0, 0, &mut budget).is_err());
        let mut budget = DecodeBudget {
            values: 0,
            expanded_bytes: MAX_EXPANDED_BYTES,
        };
        let decoder = Decoder::new(&[0x41, b'x'], 0, 2);
        assert!(matches!(
            decoder.decode_inner(0, 0, &mut budget),
            Err(Error::ResourceLimit(_))
        ));
        assert!(matches!(
            decoder.decode_inner(0, MAX_DEPTH + 1, &mut DecodeBudget::default()),
            Err(Error::ResourceLimit(_))
        ));
    }

    #[test]
    fn fast_keys_preserve_general_decoder_budgets_and_errors() {
        for data in [
            &[0x41, b'x'][..],
            &[0x20, 2, 0x41, b'x'][..],
            &[0x41, 255][..],
            &[0x5d, 0][..],
            &[0x20, 0][..],
            &[0x20, 255][..],
            &[][..],
        ] {
            for depth in [0, MAX_DEPTH, MAX_DEPTH + 1] {
                for values in [0, MAX_DECODED_VALUES] {
                    for expanded_bytes in [0, MAX_EXPANDED_BYTES] {
                        let decoder = Decoder::new(data, 0, data.len());
                        let mut a = DecodeBudget {
                            values,
                            expanded_bytes,
                        };
                        let mut b = DecodeBudget {
                            values,
                            expanded_bytes,
                        };
                        let expected = decoder
                            .decode_inner(0, depth, &mut a)
                            .map(|(value, next)| {
                                let ValueRef::Utf8(text) = value else {
                                    panic!("string fixture")
                                };
                                (text, next)
                            })
                            .map_err(|e| e.to_string());
                        let actual = decoder
                            .decode_key(0, depth, &mut b)
                            .map_err(|e| e.to_string());
                        assert_eq!(actual, expected);
                        assert_eq!((a.values, a.expanded_bytes), (b.values, b.expanded_bytes));
                    }
                }
            }
        }
    }

    #[test]
    fn truncated_scalar_payloads_fail_before_any_slice_is_borrowed() {
        for bytes in [
            &[0x68, 0x01][..],       // double: eight bytes required
            &[0x83, 0x01, 0x02][..], // byte string: three bytes declared
            &[0x04, 0x08, 0x01][..], // float: four bytes required
            &[0x42, b'a'][..],       // UTF-8 string: two bytes declared
        ] {
            assert!(matches!(
                Decoder::new(bytes, 0, bytes.len()).decode_at(0),
                Err(Error::UnexpectedEof)
            ));
        }

        // A map key must decode to text even when its value is otherwise valid.
        let invalid_key = [0xe1, 0xa1, 1, 0x41, b'x'];
        assert!(matches!(
            Decoder::new(&invalid_key, 0, invalid_key.len()).decode_at(0),
            Err(Error::DecodingError(_))
        ));

        // The map key is a pointer into the data section. The fast key path
        // must return to the inline value after resolving the pointed string.
        let pointer_key = [0xe1, 0x20, 0x05, 0x41, b'v', 0x41, b'k'];
        let (value, _) = Decoder::new(&pointer_key, 0, pointer_key.len())
            .decode_at(0)
            .unwrap();
        assert_eq!(value, ValueRef::Map(vec![("k", ValueRef::Utf8("v"))]));

        for broken in [
            [0xe1, 0x20, 0x05, 0x41, b'v', 0x42, b'k'],
            [0xe1, 0x20, 0x05, 0x41, b'v', 0x5d, 0],
        ] {
            assert!(matches!(
                Decoder::new(&broken, 0, broken.len()).decode_at(0),
                Err(Error::UnexpectedEof)
            ));
        }
    }

    #[test]
    fn utf8_validation_matches_standard_library() {
        for prefix in [0, 7, 15, 31, 127, 255] {
            for byte in 0..=255 {
                let mut bytes = vec![b'x'; prefix];
                bytes.push(byte);
                assert_eq!(utf8(&bytes).ok(), std::str::from_utf8(&bytes).ok());
            }
            for text in [
                "Paris 東京 café 🦀",
                "\u{7ff}\u{800}\u{ffff}\u{10000}\u{10ffff}",
            ] {
                let mut bytes = vec![b'x'; prefix];
                bytes.extend_from_slice(text.as_bytes());
                for end in prefix..=bytes.len() {
                    assert_eq!(
                        utf8(&bytes[..end]).ok(),
                        std::str::from_utf8(&bytes[..end]).ok()
                    );
                }
            }
        }
    }
}
