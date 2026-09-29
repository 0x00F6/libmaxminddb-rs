//! ASCII proof used before borrowed UTF-8 construction. Loads never cross the slice.

#[inline]
pub(crate) fn is_ascii(bytes: &[u8]) -> bool {
    // Each cfg block only *conditionally* returns, so control always falls
    // through to the portable scalar scan when the build target has no SIMD
    // path or the payload is too short for it.
    #[cfg(all(feature = "simd", target_arch = "x86_64"))]
    {
        // The measured crossover keeps short payloads entirely on the portable
        // path. CPU capability selection is cached once for longer payloads.
        if bytes.len() >= 64 {
            return x86_dispatch(bytes);
        }
    }
    #[cfg(all(feature = "simd", target_arch = "aarch64"))]
    {
        if bytes.len() >= 64 {
            // SAFETY: NEON is part of the aarch64 baseline.
            return unsafe { neon(bytes) };
        }
    }
    scalar(bytes)
}

#[cfg(all(feature = "simd", target_arch = "x86_64", not(target_feature = "avx2")))]
type AsciiFn = fn(&[u8]) -> bool;

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
#[inline]
fn x86_dispatch(bytes: &[u8]) -> bool {
    #[cfg(target_feature = "avx2")]
    {
        return x86_avx2_policy(bytes);
    }

    #[cfg(not(target_feature = "avx2"))]
    {
        static DISPATCH: std::sync::OnceLock<AsciiFn> = std::sync::OnceLock::new();
        let selected = *DISPATCH.get_or_init(|| {
            if std::arch::is_x86_feature_detected!("avx2") {
                x86_avx2_policy
            } else {
                x86_sse2_policy
            }
        });
        selected(bytes)
    }
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
#[inline]
fn x86_sse2_policy(bytes: &[u8]) -> bool {
    // SAFETY: SSE2 is part of the x86-64 baseline.
    unsafe { sse2(bytes) }
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
#[inline]
fn x86_avx2_policy(bytes: &[u8]) -> bool {
    if bytes.len() >= 256 {
        // SAFETY: this function is selected only when AVX2 is guaranteed either
        // by the compile target or by the one-time runtime feature check.
        unsafe { avx2(bytes) }
    } else {
        // Existing measurements keep SSE2 for the 64..255-byte range.
        x86_sse2_policy(bytes)
    }
}

/// Portable word-at-a-time reference, also used for short strings and SIMD tails.
#[inline]
pub(crate) fn scalar(mut bytes: &[u8]) -> bool {
    let len = bytes.len();
    if len <= 8 {
        return match len {
            0 => true,
            1 => (bytes[0] & 0x80) == 0,
            2 => (u16::from_ne_bytes([bytes[0], bytes[1]]) & 0x8080) == 0,
            3 => {
                let w = u16::from_ne_bytes([bytes[0], bytes[1]]);
                ((w & 0x8080) == 0) & ((bytes[2] & 0x80) == 0)
            }
            4 => (u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) & 0x8080_8080) == 0,
            _ => {
                let p = bytes.as_ptr();
                let a = unsafe { core::ptr::read_unaligned(p.cast::<u32>()) };
                let b = unsafe { core::ptr::read_unaligned(p.add(len - 4).cast::<u32>()) };
                ((a | b) & 0x8080_8080) == 0
            }
        };
    }
    if len <= 16 {
        let p = bytes.as_ptr();
        let a = unsafe { core::ptr::read_unaligned(p.cast::<u64>()) };
        let b = unsafe { core::ptr::read_unaligned(p.add(len - 8).cast::<u64>()) };
        return ((a | b) & 0x8080_8080_8080_8080) == 0;
    }
    while let Some((chunk, rest)) = bytes.split_first_chunk::<8>() {
        if u64::from_ne_bytes(*chunk) & 0x8080_8080_8080_8080 != 0 {
            return false;
        }
        bytes = rest;
    }
    if let Some((chunk, rest)) = bytes.split_first_chunk::<4>() {
        if u32::from_ne_bytes(*chunk) & 0x8080_8080 != 0 {
            return false;
        }
        bytes = rest;
    }
    if let Some((chunk, rest)) = bytes.split_first_chunk::<2>() {
        if u16::from_ne_bytes(*chunk) & 0x8080 != 0 {
            return false;
        }
        bytes = rest;
    }
    bytes.first().is_none_or(|b| b & 0x80 == 0)
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
#[target_feature(enable = "sse2")]
pub(crate) unsafe fn sse2(mut bytes: &[u8]) -> bool {
    use std::arch::x86_64::{_mm_loadu_si128, _mm_movemask_epi8, _mm_or_si128};
    while let Some((chunk, rest)) = bytes.split_first_chunk::<64>() {
        // SAFETY: four unaligned 16-byte loads stay inside this 64-byte chunk.
        let combined = unsafe {
            let p = chunk.as_ptr();
            let a = _mm_loadu_si128(p.cast());
            let b = _mm_loadu_si128(p.add(16).cast());
            let c = _mm_loadu_si128(p.add(32).cast());
            let d = _mm_loadu_si128(p.add(48).cast());
            _mm_or_si128(_mm_or_si128(a, b), _mm_or_si128(c, d))
        };
        if _mm_movemask_epi8(combined) != 0 {
            return false;
        }
        bytes = rest;
    }
    while let Some((chunk, rest)) = bytes.split_first_chunk::<16>() {
        // SAFETY: chunk contains exactly 16 readable bytes; loadu permits any alignment.
        let word = unsafe { _mm_loadu_si128(chunk.as_ptr().cast()) };
        if _mm_movemask_epi8(word) != 0 {
            return false;
        }
        bytes = rest;
    }
    scalar(bytes)
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn avx2(mut bytes: &[u8]) -> bool {
    use std::arch::x86_64::{_mm256_loadu_si256, _mm256_movemask_epi8, _mm256_or_si256};
    while let Some((chunk, rest)) = bytes.split_first_chunk::<128>() {
        // SAFETY: all four unaligned 32-byte loads are wholly inside the 128-byte chunk.
        let combined = unsafe {
            let p = chunk.as_ptr();
            let a = _mm256_loadu_si256(p.cast());
            let b = _mm256_loadu_si256(p.add(32).cast());
            let c = _mm256_loadu_si256(p.add(64).cast());
            let d = _mm256_loadu_si256(p.add(96).cast());
            _mm256_or_si256(_mm256_or_si256(a, b), _mm256_or_si256(c, d))
        };
        if _mm256_movemask_epi8(combined) != 0 {
            return false;
        }
        bytes = rest;
    }
    while let Some((chunk, rest)) = bytes.split_first_chunk::<32>() {
        // SAFETY: chunk contains 32 readable bytes, with no alignment requirement.
        let word = unsafe { _mm256_loadu_si256(chunk.as_ptr().cast()) };
        if _mm256_movemask_epi8(word) != 0 {
            return false;
        }
        bytes = rest;
    }
    scalar(bytes)
}

/// AVX-512BW ASCII scan. `_mm512_movepi8_mask` extracts the high bit of every
/// one of 64 bytes at once, so a single test covers a full cache line.
#[cfg(all(feature = "simd", target_arch = "x86_64"))]
#[allow(dead_code)]
#[target_feature(enable = "avx512f,avx512bw")]
pub(crate) unsafe fn avx512(mut bytes: &[u8]) -> bool {
    use std::arch::x86_64::{_mm512_loadu_si512, _mm512_movepi8_mask, _mm512_or_si512};
    while let Some((chunk, rest)) = bytes.split_first_chunk::<128>() {
        // SAFETY: two unaligned 64-byte loads are wholly inside the 128-byte chunk.
        let combined = unsafe {
            let p = chunk.as_ptr();
            let a = _mm512_loadu_si512(p.cast());
            let b = _mm512_loadu_si512(p.add(64).cast());
            _mm512_or_si512(a, b)
        };
        if _mm512_movepi8_mask(combined) != 0 {
            return false;
        }
        bytes = rest;
    }
    while let Some((chunk, rest)) = bytes.split_first_chunk::<64>() {
        // SAFETY: chunk contains exactly 64 readable bytes; loadu ignores alignment.
        let v = unsafe { _mm512_loadu_si512(chunk.as_ptr().cast()) };
        if _mm512_movepi8_mask(v) != 0 {
            return false;
        }
        bytes = rest;
    }
    if std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY: detection checked above.
        unsafe { avx2(bytes) }
    } else {
        // SAFETY: SSE2 is part of the x86-64 baseline.
        unsafe { sse2(bytes) }
    }
}

/// NEON ASCII scan. `vmaxvq_u8` reduces 16 bytes to their maximum, so a set high
/// bit is detected without a lane mask.
#[cfg(all(feature = "simd", target_arch = "aarch64"))]
#[target_feature(enable = "neon")]
pub(crate) unsafe fn neon(mut bytes: &[u8]) -> bool {
    use std::arch::aarch64::{vld1q_u8, vmaxvq_u8, vorrq_u8};
    while let Some((chunk, rest)) = bytes.split_first_chunk::<64>() {
        // SAFETY: four unaligned 16-byte loads are wholly inside the 64-byte chunk.
        let combined = unsafe {
            let p = chunk.as_ptr();
            let a = vld1q_u8(p);
            let b = vld1q_u8(p.add(16));
            let c = vld1q_u8(p.add(32));
            let d = vld1q_u8(p.add(48));
            vorrq_u8(vorrq_u8(a, b), vorrq_u8(c, d))
        };
        if vmaxvq_u8(combined) & 0x80 != 0 {
            return false;
        }
        bytes = rest;
    }
    while let Some((chunk, rest)) = bytes.split_first_chunk::<16>() {
        // SAFETY: chunk contains exactly 16 readable bytes; vld1q_u8 ignores alignment.
        let v = unsafe { vld1q_u8(chunk.as_ptr()) };
        if vmaxvq_u8(v) & 0x80 != 0 {
            return false;
        }
        bytes = rest;
    }
    scalar(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(bytes: &[u8]) {
        let expected = bytes.is_ascii();
        assert_eq!(scalar(bytes), expected);
        assert_eq!(is_ascii(bytes), expected);
        #[cfg(all(feature = "simd", target_arch = "x86_64"))]
        {
            // SAFETY: x86-64 always supports SSE2.
            assert_eq!(unsafe { sse2(bytes) }, expected);
            if std::arch::is_x86_feature_detected!("avx2") {
                // SAFETY: checked AVX2 support above.
                assert_eq!(unsafe { avx2(bytes) }, expected);
            }
            if std::arch::is_x86_feature_detected!("avx512f")
                && std::arch::is_x86_feature_detected!("avx512bw")
            {
                // SAFETY: checked AVX-512F/BW support above.
                assert_eq!(unsafe { avx512(bytes) }, expected);
            }
        }
    }

    #[test]
    fn every_alignment_tail_and_high_bit_position() {
        let mut data = vec![b'x'; 64 + 257];
        for alignment in 0..64 {
            for len in 0..=257 {
                check(&data[alignment..alignment + len]);
                for pos in 0..len {
                    data[alignment + pos] = 0x80 | (pos as u8);
                    check(&data[alignment..alignment + len]);
                    data[alignment + pos] = b'x';
                }
            }
        }
    }
}
