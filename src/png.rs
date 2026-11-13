//! PNG files as this converter writes them: RGBA8, filter 0 on every row, one IDAT (zlib level 9).

use crate::zlib::{compress9, crc32_update};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

/// The same texture page is written many times (both forms of a costume, every stage variant, each debris
/// piece): while a conversion runs, PNGs are kept by content (size and a CRC-32 and a 64-bit hash of the pixels).
struct Cache {
    users: usize,
    pngs: BTreeMap<(usize, usize, u64, u64), Vec<u8>>,
    bytes: usize,
}

/// The cache is emptied when it grows past this (the repeats are close together: a costume's two forms, a stage's
/// variants).
const CACHE_LIMIT: usize = 32 << 20;

static CACHE: Mutex<Cache> = Mutex::new(Cache { users: 0, pngs: BTreeMap::new(), bytes: 0 });

fn key(w: usize, h: usize, rgba: &[u8]) -> (usize, usize, u64, u64) {
    let mut sip = std::hash::DefaultHasher::new();
    rgba.hash(&mut sip);
    (w, h, crate::zlib::crc32(rgba) as u64, sip.finish())
}

/// A conversion starts using the cache.
pub fn cache_begin() {
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).users += 1;
}

/// A conversion is done with it; the last one empties it.
pub fn cache_end() {
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    c.users = c.users.saturating_sub(1);
    if c.users == 0 {
        c.pngs = BTreeMap::new();
        c.bytes = 0;
    }
}

fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(body);
    let c = crc32_update(crc32_update(0, tag), body);
    out.extend_from_slice(&c.to_be_bytes());
}

pub fn png_bytes(w: usize, h: usize, rgba: &[u8]) -> Vec<u8> {
    let k = key(w, h, rgba);
    {
        let c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if c.users > 0
            && let Some(p) = c.pngs.get(&k)
        {
            return p.clone();
        }
    }
    let out = encode(w, h, rgba);
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if c.users > 0 {
        if c.bytes + out.len() > CACHE_LIMIT {
            c.pngs = BTreeMap::new();
            c.bytes = 0;
        }
        c.bytes += out.len();
        c.pngs.insert(k, out.clone());
    }
    out
}

fn encode(w: usize, h: usize, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(h * (w * 4 + 1));
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    let z = compress9(&raw);
    let mut out = Vec::with_capacity(z.len() + 64);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", b"");
    out
}
