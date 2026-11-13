//! The game's LZ compression and its containers.
//!
//! Container (`*.DAT`, `*.ACT`, …): `u32 n, u32 off[n]` (relative to the table end, `0xFFFFFFFF` = empty slot);
//! entry = `u32 type (2 = compressed), u32 packed size, u32 unpacked size, data`.
//! LZ: flag bits MSB first, the flag byte fetched when needed. `1` literal; `0 0 b b` + byte o: copy `bb + 2`
//! from `o - 256`; `0 1` + `b0 b1`: offset `(b0 << 5 | b1 >> 3) - 0x2000`, length `(b1 & 7) + 2`, or with
//! `b1 & 7 == 0` an extra byte c: 0 = end, else length `c + 1`.

#[derive(Debug)]
pub struct LzError(pub &'static str);

/// Decompresses from `src[pos..]` until `out_size` bytes are out (or the end code). Returns the data and the
/// position after the compressed stream.
pub fn decompress(src: &[u8], mut pos: usize, out_size: Option<usize>) -> Result<(Vec<u8>, usize), LzError> {
    let mut out: Vec<u8> = Vec::with_capacity(out_size.unwrap_or(0));
    let (mut flags, mut nbits) = (0u32, 0u32);
    let byte = |pos: &mut usize| -> Result<u8, LzError> {
        let b = *src.get(*pos).ok_or(LzError("compressed data truncated"))?;
        *pos += 1;
        Ok(b)
    };
    macro_rules! bit {
        () => {{
            if nbits == 0 {
                flags = byte(&mut pos)? as u32;
                nbits = 8;
            }
            nbits -= 1;
            (flags >> nbits) & 1
        }};
    }
    while out_size.is_none_or(|n| out.len() < n) {
        if bit!() == 1 {
            out.push(byte(&mut pos)?);
            continue;
        }
        let (off, ln): (i64, usize);
        if bit!() == 0 {
            let hi = bit!();
            let lo = bit!();
            ln = ((hi << 1) | lo) as usize + 2;
            off = byte(&mut pos)? as i64 - 256;
        } else {
            let b0 = byte(&mut pos)? as i64;
            let b1 = byte(&mut pos)? as i64;
            off = ((b0 << 5) | (b1 >> 3)) - 0x2000;
            let n = (b1 & 7) as usize;
            if n != 0 {
                ln = n + 2;
            } else {
                let c = byte(&mut pos)?;
                if c == 0 {
                    break;
                }
                ln = c as usize + 1;
            }
        }
        let s = out.len() as i64 + off;
        if s < 0 {
            return Err(LzError("offset before start"));
        }
        for i in 0..ln {
            let b = out[s as usize + i];
            out.push(b);
        }
    }
    Ok((out, pos))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// Every entry of a container, decompressed (`None` for empty slots). Errors on a malformed
/// container (the callers skip such files).
pub fn read_container(data: &[u8]) -> Result<Vec<Option<Vec<u8>>>, LzError> {
    let n = u32_at(data, 0).ok_or(LzError("short container"))? as usize;
    let base = 4 + 4 * n;
    if base > data.len() {
        return Err(LzError("container table past the end"));
    }
    let mut res = Vec::with_capacity(n);
    for i in 0..n {
        let o = u32_at(data, 4 + 4 * i).unwrap();
        if o == 0xFFFF_FFFF {
            res.push(None);
            continue;
        }
        let e = base + o as usize;
        let (t, usz) = match (u32_at(data, e), u32_at(data, e + 8)) {
            (Some(t), Some(u)) => (t, u as usize),
            _ => return Err(LzError("entry header past the end")),
        };
        if t == 2 {
            res.push(Some(decompress(data, e + 12, Some(usz))?.0));
        } else {
            // clipped at the end of the data
            let s = (e + 12).min(data.len());
            res.push(Some(data[s..(e + 12 + usz).min(data.len())].to_vec()));
        }
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_short_and_long_matches() {
        // flags 1 1 0 0 0 1 | 0 1 ... : "ab", short copy len 3 from -2, long copy, end
        // byte 0: 1,1 literal a,b; 0,0,0,1 -> short match len (0<<1|1)+2 = 3, offset byte 0xFE (-2)
        // then bits 0,1 -> long: b0 b1 with off = -2 => (b0<<5|b1>>3) = 0x1FFE, b1&7 = 2 -> len 4
        //   0x1FFE = b0<<5 | b1>>3 -> b0 = 0xFF, b1>>3 = 0x1E -> b1 = 0xF2
        // then 0,1 long with b1&7 = 0 and c = 0: end -> b0 = 0, b1 = 0, c = 0
        let src = [0b1100_0101, b'a', b'b', 0xFE, 0xFF, 0xF2, 0b0100_0000, 0x00, 0x00, 0x00];
        let (out, end) = decompress(&src, 0, None).unwrap();
        assert_eq!(out, b"abababababa".to_vec()[..9].to_vec());
        assert_eq!(end, src.len());
    }

    #[test]
    fn container_entries() {
        // two entries: raw "xyz" and an empty slot
        let mut d = vec![2, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF];
        d.extend_from_slice(&[0, 0, 0, 0, 3, 0, 0, 0, 3, 0, 0, 0, b'x', b'y', b'z']);
        let e = read_container(&d).unwrap();
        assert_eq!(e, vec![Some(b"xyz".to_vec()), None]);
    }
}
