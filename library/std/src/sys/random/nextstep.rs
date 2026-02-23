//! Random data generation for NeXTSTEP using 4.3BSD `random()`/`srandom()`.
//!
//! Not cryptographically secure, but adequate for HashMap DoS protection
//! and general PRNG use. Seeded once with `gettimeofday() ^ getpid()`.

use crate::sync::Once;

static SEED_ONCE: Once = Once::new();

fn ensure_seeded() {
    SEED_ONCE.call_once(|| {
        let mut tv = nextstep_sys::timeval { tv_sec: 0, tv_usec: 0 };
        unsafe { nextstep_sys::gettimeofday(&mut tv, core::ptr::null_mut()) };
        let pid = unsafe { nextstep_sys::getpid() };
        let seed = (tv.tv_sec as u32) ^ (tv.tv_usec as u32) ^ (pid as u32);
        unsafe { nextstep_sys::srandom(seed) };
    });
}

pub fn fill_bytes(bytes: &mut [u8]) {
    ensure_seeded();
    for chunk in bytes.chunks_mut(4) {
        let val = unsafe { nextstep_sys::random() } as u32;
        let val_bytes = val.to_ne_bytes();
        for (i, b) in chunk.iter_mut().enumerate() {
            *b = val_bytes[i];
        }
    }
}

pub fn hashmap_random_keys() -> (u64, u64) {
    let mut buf = [0u8; 16];
    fill_bytes(&mut buf);
    let k1 = u64::from_ne_bytes(buf[..8].try_into().unwrap());
    let k2 = u64::from_ne_bytes(buf[8..].try_into().unwrap());
    (k1, k2)
}
