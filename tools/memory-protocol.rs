//! Shared fixture/workload definition for the RSS comparison (not core library code).
pub const PROTOCOL: &str = "memory-rss-v2";
pub const LOOKUPS: usize = 1_000_000;
pub const WARMUP: usize = 1_000;
pub const SEED: u64 = 0x5eed_2024_cafe_babe;

/// A permutation modulo 2^31 followed by a left shift. All routes are distinct
/// even /32 addresses; odd neighbours are guaranteed misses. Odd multipliers
/// and xor-shifts are invertible, so no set or collision rejection is needed.
pub fn route(index: u32) -> u32 {
    let mut x = index ^ (index >> 16);
    x = x.wrapping_mul(0x045d_9f3b) & 0x7fff_ffff;
    x ^= x >> 15;
    x = x.wrapping_mul(0x119d_e1f3) & 0x7fff_ffff;
    (x ^ (x >> 16)) << 1
}

pub fn workload(entries: usize, count: usize) -> Vec<u8> {
    assert!(entries > 0 && entries <= 0x8000_0000);
    assert!(count.is_multiple_of(2));
    let mut state = SEED;
    let mut bytes = Vec::with_capacity(count * 4);
    for _ in 0..count / 2 {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        let ip = route(((z ^ (z >> 31)) % entries as u64) as u32);
        bytes.extend_from_slice(&ip.to_be_bytes());
        bytes.extend_from_slice(&(ip | 1).to_be_bytes());
    }
    bytes
}
