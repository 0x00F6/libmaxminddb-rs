//! Metadata-marker search over the final 128 KiB window.
//!
//! The tree and the data section are located by the trailing
//! `\xab\xcd\xefMaxMind.com` marker. Opening a database scans up to 128 KiB for it.
//!
//! The marker is emitted at the end of the metadata and is therefore almost always
//! within the final few hundred bytes of the window. Scanning **backwards** from the
//! end finds it after a handful of chunk loads, so a portable reversed scan is the
//! reference implementation and the SIMD kernels chunk the window from the top down.
//! (A forward full-window SIMD sweep was measured ~40-80x slower on the canonical
//! `GeoIP2-City-Bench.mmdb` window, so it was rejected.)
//!
//! All candidate positions are verified with a plain 14-byte slice compare, which
//! LLVM lowers to a single vectorized compare.

use crate::Result;

pub(crate) const METADATA_MARKER: &[u8] = b"\xab\xcd\xefMaxMind.com";
pub(crate) const MAX_METADATA_SIZE: usize = 128 * 1024;

/// Finds the last metadata marker in the final 128 KiB of `data`.
///
/// Returns the absolute offset of the marker. Only the final
/// [`MAX_METADATA_SIZE`] bytes are searched, preserving the hardened bound.
#[allow(clippy::unnecessary_lazy_evaluations)]
pub(crate) fn find_metadata_marker(data: &[u8]) -> Result<usize> {
    let start = data.len().saturating_sub(MAX_METADATA_SIZE);
    let window = &data[start..];
    let rel = last_marker_in_window_dispatch(window).ok_or_else(|| {
        crate::Error::InvalidMetadata("metadata marker not found in final 128 KiB")
    })?;
    Ok(start + rel)
}

/// Portable reference: scans backwards and returns the last verified match.
///
/// This is also used for sub-16-byte heads of the SIMD kernels. It is the spec:
/// return the greatest `i` in `0..=window.len()-14` whose 14 bytes equal
/// `METADATA_MARKER`, or `None`.
#[inline]
pub(crate) fn last_marker_in_window(window: &[u8]) -> Option<usize> {
    if window.len() < METADATA_MARKER.len() {
        return None;
    }
    let valid_end = window.len() - METADATA_MARKER.len();
    // 0xab is the marker's first byte; rare in the metadata area, so first
    // locating it beats comparing 14 bytes at every offset. Scanning backwards
    // finds the marker near EOF immediately.
    (0..=valid_end)
        .rev()
        .find(|&i| window[i] == METADATA_MARKER[0] && window[i..i + 14] == *METADATA_MARKER)
}

/// Scalar backward scan over candidate starts `0..limit`, verifying against the
/// full `window`. Used for the sub-chunk head that the SIMD loops leave behind.
#[cfg(all(feature = "simd", any(target_arch = "x86_64", target_arch = "aarch64")))]
#[inline]
fn scalar_backward_range(window: &[u8], limit: usize, valid_end: usize) -> Option<usize> {
    let mut p = limit.min(valid_end + 1);
    while p > 0 {
        p -= 1;
        if window[p] == METADATA_MARKER[0] && window[p..p + 14] == *METADATA_MARKER {
            return Some(p);
        }
    }
    None
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
pub(crate) fn last_marker_in_window_dispatch(window: &[u8]) -> Option<usize> {
    if window.len() < METADATA_MARKER.len() {
        return None;
    }
    // The retained canonical-window microbenchmark measured the backward SSE2
    // kernel at ~6 ns versus ~95 ns scalar. Wider backward kernels have no
    // retained measurement, so production dispatch deliberately uses the
    // measured x86-64 baseline instead of guessing that wider is faster.
    // SAFETY: SSE2 is part of the x86-64 baseline.
    unsafe { marker_sse2(window) }
}

#[cfg(all(feature = "simd", target_arch = "aarch64"))]
pub(crate) fn last_marker_in_window_dispatch(window: &[u8]) -> Option<usize> {
    if window.len() < METADATA_MARKER.len() {
        return None;
    }
    // SAFETY: NEON is part of the aarch64 baseline.
    unsafe { marker_neon(window) }
}

#[cfg(any(
    not(feature = "simd"),
    all(
        feature = "simd",
        not(any(target_arch = "x86_64", target_arch = "aarch64"))
    )
))]
pub(crate) fn last_marker_in_window_dispatch(window: &[u8]) -> Option<usize> {
    // Portable fallback for architectures without a retained explicit SIMD kernel.
    last_marker_in_window(window)
}

/// Verifies the 14-byte marker at `window[i..i+14]`; caller guarantees in bounds.
#[cfg(all(feature = "simd", any(target_arch = "x86_64", target_arch = "aarch64")))]
#[inline(always)]
fn verify(window: &[u8], i: usize) -> bool {
    window[i..i + METADATA_MARKER.len()] == *METADATA_MARKER
}

/// Returns `true` and the highest set bit of `mask` if any, scanning candidates
/// of a backward chunk from `base` downwards. Only positions `<= valid_end` can
/// hold a full marker.
#[cfg(all(feature = "simd", any(target_arch = "x86_64", target_arch = "aarch64")))]
#[inline(always)]
fn best_candidate(window: &[u8], base: usize, valid_end: usize, mut mask: u32) -> Option<usize> {
    while mask != 0 {
        let bit = 31 - mask.leading_zeros() as usize;
        mask &= !(1_u32 << bit);
        let pos = base + bit;
        if pos <= valid_end && verify(window, pos) {
            return Some(pos);
        }
    }
    None
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
#[target_feature(enable = "sse2")]
pub(crate) unsafe fn marker_sse2(window: &[u8]) -> Option<usize> {
    use std::arch::x86_64::{_mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8};
    if window.len() < METADATA_MARKER.len() {
        return None;
    }
    let valid_end = window.len() - METADATA_MARKER.len();
    if window.len() < 16 {
        return last_marker_in_window(window);
    }
    let needle = _mm_set1_epi8(METADATA_MARKER[0] as i8);
    let ptr = window.as_ptr();
    // SAFETY: `i + 16 <= window.len()` for every visited i, so each load is in
    // bounds; `pos <= valid_end` is enforced before the marker slice compare.
    let mut i = window.len() - 16;
    loop {
        // SAFETY: `i + 16 <= window.len()` is preserved by the loop, so the load
        // is in bounds; `pos <= valid_end` is enforced inside `best_candidate`.
        let v = unsafe { _mm_loadu_si128(ptr.add(i).cast()) };
        let mask = _mm_movemask_epi8(_mm_cmpeq_epi8(v, needle)) as u32;
        if let Some(pos) = best_candidate(window, i, valid_end, mask) {
            return Some(pos);
        }
        if i < 16 {
            break;
        }
        i -= 16;
    }
    scalar_backward_range(window, i, valid_end)
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
#[allow(dead_code)]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn marker_avx2(window: &[u8]) -> Option<usize> {
    use std::arch::x86_64::{
        _mm256_cmpeq_epi8, _mm256_loadu_si256, _mm256_movemask_epi8, _mm256_set1_epi8,
    };
    if window.len() < METADATA_MARKER.len() {
        return None;
    }
    let valid_end = window.len() - METADATA_MARKER.len();
    if window.len() < 32 {
        // SAFETY: SSE2 is the x86-64 baseline.
        return unsafe { marker_sse2(window) };
    }
    let needle = _mm256_set1_epi8(METADATA_MARKER[0] as i8);
    let ptr = window.as_ptr();
    // SAFETY: `i + 32 <= window.len()` is preserved by the loop, so the load is
    // in bounds; `pos <= valid_end` is enforced inside `best_candidate`.
    let mut i = window.len() - 32;
    loop {
        let v = unsafe { _mm256_loadu_si256(ptr.add(i).cast()) };
        let mask = _mm256_movemask_epi8(_mm256_cmpeq_epi8(v, needle)) as u32;
        if let Some(pos) = best_candidate(window, i, valid_end, mask) {
            return Some(pos);
        }
        if i < 32 {
            break;
        }
        i -= 32;
    }
    scalar_backward_range(window, i, valid_end)
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
#[allow(dead_code)]
#[target_feature(enable = "avx512f,avx512bw")]
pub(crate) unsafe fn marker_avx512(window: &[u8]) -> Option<usize> {
    use std::arch::x86_64::{_mm512_cmpeq_epi8_mask, _mm512_loadu_si512, _mm512_set1_epi8};
    if window.len() < METADATA_MARKER.len() {
        return None;
    }
    let valid_end = window.len() - METADATA_MARKER.len();
    if window.len() < 64 {
        if std::arch::is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 was detected at runtime.
            return unsafe { marker_avx2(window) };
        }
        // SAFETY: SSE2 is part of the x86-64 baseline.
        return unsafe { marker_sse2(window) };
    }
    let needle = _mm512_set1_epi8(METADATA_MARKER[0] as i8);
    let ptr = window.as_ptr();
    // SAFETY: `i + 64 <= window.len()` is preserved by the loop, so the load is
    // in bounds; `pos <= valid_end` is enforced before the marker slice compare.
    let mut i = window.len() - 64;
    loop {
        let v = unsafe { _mm512_loadu_si512(ptr.add(i).cast()) };
        let mut mask = _mm512_cmpeq_epi8_mask(v, needle);
        while mask != 0 {
            let bit = 63 - mask.leading_zeros() as usize;
            mask &= !(1_u64 << bit);
            let pos = i + bit;
            if pos <= valid_end && verify(window, pos) {
                return Some(pos);
            }
        }
        if i < 64 {
            break;
        }
        i -= 64;
    }
    scalar_backward_range(window, i, valid_end)
}

#[cfg(all(feature = "simd", target_arch = "aarch64"))]
#[target_feature(enable = "neon")]
pub(crate) unsafe fn marker_neon(window: &[u8]) -> Option<usize> {
    use std::arch::aarch64::{vceqq_u8, vdupq_n_u8, vld1q_u8};
    if window.len() < METADATA_MARKER.len() {
        return None;
    }
    let valid_end = window.len() - METADATA_MARKER.len();
    if window.len() < 16 {
        return last_marker_in_window(window);
    }
    let needle = vdupq_n_u8(METADATA_MARKER[0]);
    let ptr = window.as_ptr();
    let mut i = window.len() - 16;
    loop {
        // SAFETY: `i + 16 <= window.len()` keeps the load in bounds; `pos <=
        // valid_end` is enforced before the marker slice compare.
        let v = unsafe { vld1q_u8(ptr.add(i)) };
        let eq = vceqq_u8(v, needle);
        // Per-lane mask (each byte 0xff/0x00); available with NEON without an
        // x86-style movemask. Scan lanes from most significant to least.
        let bytes: [u8; 16] = core::mem::transmute_copy(&eq);
        for bit in (0..16).rev() {
            if bytes[bit] != 0 {
                let pos = i + bit;
                if pos <= valid_end && verify(window, pos) {
                    return Some(pos);
                }
            }
        }
        if i < 16 {
            break;
        }
        i -= 16;
    }
    scalar_backward_range(window, i, valid_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aligned_tail_check(data: &[u8]) {
        let expected = last_marker_in_window(data);
        let actual = last_marker_in_window_dispatch(data);
        assert_eq!(actual, expected, "dispatch len {}", data.len());
        #[cfg(all(feature = "simd", target_arch = "x86_64"))]
        {
            // SAFETY: SSE2 is the x86-64 baseline.
            assert_eq!(
                unsafe { marker_sse2(data) },
                expected,
                "sse2 len {}",
                data.len()
            );
            if std::arch::is_x86_feature_detected!("avx2") {
                // SAFETY: checked AVX2 above.
                assert_eq!(
                    unsafe { marker_avx2(data) },
                    expected,
                    "avx2 len {}",
                    data.len()
                );
            }
            if std::arch::is_x86_feature_detected!("avx512f")
                && std::arch::is_x86_feature_detected!("avx512bw")
            {
                // SAFETY: checked AVX-512F/BW above.
                assert_eq!(
                    unsafe { marker_avx512(data) },
                    expected,
                    "avx512 len {}",
                    data.len()
                );
            }
        }
    }

    #[test]
    fn finds_marker_across_lengths_and_marker_positions() {
        for marker_pos in 0..300 {
            for garbage_len in [0_usize, 10, 14, 15, 16, 17, 64, 100] {
                let mut data = vec![0x42; garbage_len];
                let mut mark = Vec::new();
                mark.extend_from_slice(METADATA_MARKER);
                while mark.len() < marker_pos {
                    mark.push(0x43);
                }
                data.extend_from_slice(&mark);
                assert_eq!(last_marker_in_window(&data), Some(garbage_len));
                aligned_tail_check(&data);
            }
        }
    }

    #[test]
    fn last_of_multiple_markers_wins() {
        let mut data = Vec::new();
        data.extend_from_slice(METADATA_MARKER);
        data.extend_from_slice(&[0x42; 33]);
        data.extend_from_slice(METADATA_MARKER);
        data.extend_from_slice(&[0x42; 7]);
        let expected = Some(47);
        assert_eq!(last_marker_in_window(&data), expected);
        aligned_tail_check(&data);
        // Trailing bytes after the marker keep the last marker position.
        data.extend_from_slice(&[0x42; 5]);
        assert_eq!(last_marker_in_window(&data), expected);
        aligned_tail_check(&data);
    }

    #[test]
    fn no_marker_returns_none() {
        for len in [0_usize, 1, 13, 14, 15, 16, 31, 32, 63, 64, 100, 300] {
            let data = vec![0x42; len];
            assert_eq!(last_marker_in_window(&data), None);
            aligned_tail_check(&data);
        }
        // A partial marker (first byte only) must not match.
        let mut data = vec![0x42; 40];
        data[17] = METADATA_MARKER[0];
        assert_eq!(last_marker_in_window(&data), None);
        aligned_tail_check(&data);
    }

    #[test]
    fn forged_first_byte_candidates_are_rejected() {
        // Sprinkle 0xab bytes (marker first byte) that do not continue the marker.
        let mut data = vec![0x42; 700];
        let mut rng = 0x1234_5678_u32;
        for i in (0..data.len()).step_by(7) {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            data[i] = (rng >> 24) as u8;
        }
        data[650] = METADATA_MARKER[0];
        data[650..650 + METADATA_MARKER.len()].copy_from_slice(METADATA_MARKER);
        assert_eq!(last_marker_in_window(&data), Some(650));
        aligned_tail_check(&data);

        // A marker prefix near the end cannot contain the full marker. SIMD
        // candidates must apply the same last-valid-start bound as the scalar scan.
        let mut tail = vec![0x42; 80];
        tail[79] = METADATA_MARKER[0];
        aligned_tail_check(&tail);
    }

    #[test]
    fn oversized_window_behaviour_matches_scalar() {
        // Long window (>= 64 bytes) so the AVX-512 chunk loop runs and the
        // AVX2/SSE2 heads are exercised too; the marker sits inside the window.
        let mut data = vec![0x55; 5000];
        data.extend_from_slice(METADATA_MARKER);
        data.extend_from_slice(&[0x33; 128]);
        let window = data.as_slice();
        let expected = Some(5000);
        assert_eq!(last_marker_in_window(window), expected);
        aligned_tail_check(window);
        // Window trimmed so the marker starts exactly at relative offset 0.
        let window = &data[5000..];
        assert_eq!(last_marker_in_window(window), Some(0));
        aligned_tail_check(window);
        // Marker ending exactly at the last valid start position (tail boundary).
        let mut data2 = vec![0x55; 4096];
        data2.extend_from_slice(METADATA_MARKER);
        data2.extend_from_slice(&[0x33; 64]);
        let window = &data2[..4096 + METADATA_MARKER.len()];
        let expected = Some(4096);
        assert_eq!(last_marker_in_window(window), expected);
        aligned_tail_check(window);
    }

    #[test]
    fn marker_in_sub_chunk_head_is_found() {
        // Marker starting within the sub-chunk head the SIMD loops leave behind
        // (positions below `len % chunk`), which must still be scanned.
        for chunk in [16_usize, 32, 64] {
            let mut data = vec![0x42; chunk + 20];
            data[2] = METADATA_MARKER[0];
            data[2..2 + METADATA_MARKER.len()].copy_from_slice(METADATA_MARKER);
            assert_eq!(last_marker_in_window(&data), Some(2), "chunk {chunk}");
            aligned_tail_check(&data);
        }
    }
}
