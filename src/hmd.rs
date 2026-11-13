//! Sony HMD models as Bloody Roar II stores them.
//!
//! All pointers are u32 word offsets from the HMD start (bit 31 = "relative"): `u32 id (0x50), flags,
//! prim_header_ptr, nblock, block_ptr[nblock]`. The primitive header section: `u32 nheader`, then headers `u32 nptr,
//! ptr[nptr]` — regular `[poly data, vertices, normals, coordinates]`, shared `[poly data, source vertices, work
//! vertices, source normals, work normals]`. Coordinates: `u32 n`, GsCOORDINATE2[n] (0x50 bytes). Block i (1..n)
//! belongs to coordinate i - 1. A primitive set: `u32 next, header index, ntype | 0x80000000`, then per type
//! `u32 type, u32 (0x80000000 | count << 16 | words)` and `words - 1` argument words.

use std::collections::BTreeMap;

fn w(d: &[u8], a: usize) -> Result<u32, String> {
    d.get(a..a + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| format!("HMD read past the end at {a:#x}"))
}
fn h16(d: &[u8], a: usize) -> Result<u16, String> {
    d.get(a..a + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(|| format!("HMD read past the end at {a:#x}"))
}

#[derive(Clone, Debug)]
pub struct Prim {
    pub verts: Vec<u16>,
    pub norms: Option<Vec<u16>>,
    pub uvs: Option<Vec<(u8, u8)>>,
    pub clut: u32,
    pub tpage: u32,
    pub color: Option<Vec<[u8; 3]>>,
    pub bone: i64,
    pub shared: bool,
    pub quad: bool,
    pub semi: bool,
}

/// Size in words of one polygon packet of the given type (low byte).
pub fn packet_words(ptype: u32) -> usize {
    let (tme, iip, quad, nolight) = (ptype & 1, (ptype >> 2) & 1, (ptype >> 4) & 1, (ptype >> 6) & 1);
    let nv: usize = if quad != 0 { 4 } else { 3 };
    let colors = if nolight != 0 {
        if iip != 0 { nv } else { 1 }
    } else if tme != 0 {
        0
    } else {
        1
    };
    let need = if nolight != 0 {
        nv
    } else if iip != 0 {
        2 * nv
    } else {
        nv + 1
    };
    let pads = if tme != 0 { if quad != 0 { 2 } else { 1 } } else { 0 };
    colors + if tme != 0 { nv } else { 0 } + need.saturating_sub(pads).div_ceil(2)
}

fn decode_polys(d: &[u8], ptype: u32, count: usize, base: usize) -> Result<Vec<Prim>, String> {
    let (tme, iip, quad, nolight) = (ptype & 1, (ptype >> 2) & 1, (ptype >> 4) & 1, (ptype >> 6) & 1);
    let nv: usize = if quad != 0 { 4 } else { 3 };
    let ncol = if nolight != 0 {
        if iip != 0 { nv } else { 1 }
    } else if tme != 0 {
        0
    } else {
        1
    };
    let need = if nolight != 0 {
        nv
    } else if iip != 0 {
        2 * nv
    } else {
        nv + 1
    };
    let size = packet_words(ptype);
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        let mut a = base + k * size * 4;
        let mut cols = Vec::new();
        for i in 0..ncol {
            let c = d.get(a + 4 * i..a + 4 * i + 3).ok_or("HMD colour past the end")?;
            cols.push([c[0], c[1], c[2]]);
        }
        a += 4 * ncol;
        let mut p = Prim {
            verts: Vec::new(),
            norms: None,
            uvs: None,
            clut: 0,
            tpage: 0,
            color: if cols.is_empty() { None } else { Some(cols) },
            bone: 0,
            shared: false,
            quad: quad != 0,
            semi: false,
        };
        let mut hw: Vec<u16> = Vec::new();
        if tme != 0 {
            let mut uvw = Vec::with_capacity(nv);
            for i in 0..nv {
                uvw.push(w(d, a + 4 * i)?);
            }
            p.uvs = Some(uvw.iter().map(|&x| ((x & 0xFF) as u8, ((x >> 8) & 0xFF) as u8)).collect());
            p.clut = uvw[0] >> 16;
            p.tpage = uvw[1] >> 16;
            hw.push((uvw[2] >> 16) as u16);
            if quad != 0 {
                hw.push((uvw[3] >> 16) as u16);
            }
            a += 4 * nv;
        }
        let rest = size - ncol - if tme != 0 { nv } else { 0 };
        for i in 0..2 * rest {
            hw.push(h16(d, a + 2 * i)?);
        }
        let hw: Vec<u16> = if tme != 0 { hw[hw.len() - need..].to_vec() } else { hw[..need.min(hw.len())].to_vec() };
        if nolight != 0 {
            p.verts = hw;
        } else if iip != 0 {
            p.norms = Some(hw.iter().step_by(2).copied().collect());
            p.verts = hw.iter().skip(1).step_by(2).copied().collect();
        } else {
            p.norms = Some(vec![hw[0]; nv]);
            p.verts = hw[1..].to_vec();
        }
        out.push(p);
    }
    Ok(out)
}

#[derive(Clone, Debug)]
pub struct Coord {
    pub m: [i64; 9],
    pub t: [i64; 3],
}

/// One vertex-calculation (skinning) record: work[dst..] = M_bone * src[src..].
#[derive(Clone, Debug)]
pub struct Calc {
    pub bone: i64,
    pub vc: u32,
    pub vs: u32,
    pub vd: u32,
    pub nc: u32,
    pub ns: u32,
    pub nd: u32,
    pub hidx: u32,
}

pub struct Hmd<'a> {
    pub d: &'a [u8],
    /// header word index → pointers (in file order)
    pub headers: BTreeMap<u32, Vec<u32>>,
    pub header_order: Vec<u32>,
    pub coords: Vec<Coord>,
    pub calcs: Vec<Calc>,
    pub polys: Vec<(Prim, u32)>,
}

impl<'a> Hmd<'a> {
    pub fn parse(d: &'a [u8]) -> Result<Hmd<'a>, String> {
        let (hid, ph, nblock) = (w(d, 0)?, w(d, 8)? as usize, w(d, 12)? as usize);
        if hid != 0x50 {
            return Err(format!("not an HMD (id {hid:x})"));
        }
        let mut blocks = Vec::with_capacity(nblock);
        for i in 0..nblock {
            blocks.push(w(d, 16 + 4 * i)? & 0x7FFF_FFFF);
        }
        let mut headers = BTreeMap::new();
        let mut header_order = Vec::new();
        let nh = w(d, ph * 4)?;
        let mut a = ph * 4 + 4;
        for _ in 0..nh {
            let n = w(d, a)? as usize;
            let mut v = Vec::with_capacity(n);
            for i in 0..n {
                v.push(w(d, a + 4 + 4 * i)? & 0x7FFF_FFFF);
            }
            let key = (a / 4) as u32;
            if headers.insert(key, v).is_none() {
                header_order.push(key);
            }
            a += 4 + 4 * n;
        }
        let mut coords = Vec::new();
        let mut cptr = None;
        for k in &header_order {
            let h = &headers[k];
            if h.len() == 4 {
                cptr = Some(h[3] as usize);
            }
        }
        if let Some(cptr) = cptr {
            let nc = w(d, cptr * 4)? as usize;
            for i in 0..nc {
                let o = cptr * 4 + 4 + i * 0x50;
                let mut m = [0i64; 9];
                for (j, x) in m.iter_mut().enumerate() {
                    *x = h16(d, o + 4 + 2 * j)? as i16 as i64;
                }
                let mut t = [0i64; 3];
                for (j, x) in t.iter_mut().enumerate() {
                    *x = w(d, o + 0x18 + 4 * j)? as i32 as i64;
                }
                w(d, o + 0x48)?;
                coords.push(Coord { m, t });
            }
        }
        let mut h = Hmd { d, headers, header_order, coords, calcs: Vec::new(), polys: Vec::new() };
        h.parse_blocks(&blocks)?;
        Ok(h)
    }

    fn parse_blocks(&mut self, blocks: &[u32]) -> Result<(), String> {
        let d = self.d;
        for (bi, &bp) in blocks.iter().enumerate() {
            if bp == 0 {
                continue;
            }
            let mut p = bp;
            let mut seen = std::collections::HashSet::new();
            while p != 0x7FFF_FFFF && seen.insert(p) {
                let mut a = p as usize * 4;
                let (nxt, hidx, nt) = (w(d, a)?, w(d, a + 4)?, w(d, a + 8)? & 0x7FFF_FFFF);
                let hdr = self.headers.get(&hidx).cloned();
                a += 12;
                for _ in 0..nt {
                    let (ptype, sc) = (w(d, a)?, w(d, a + 4)?);
                    let nwords = (sc & 0xFFFF) as usize;
                    let count = ((sc >> 16) & 0x7FFF) as usize;
                    let mut args = Vec::new();
                    if nwords > 1 {
                        for i in 0..nwords - 1 {
                            args.push(w(d, a + 8 + 4 * i)?);
                        }
                    }
                    a += 4 + 4 * nwords;
                    let cat = ptype >> 24;
                    let low = ptype & 0xFF_FFFF;
                    let bone = bi as i64 - 1;
                    if cat == 1 && (low & 0x7F) == 0 {
                        if args.len() < 6 {
                            return Err("short vertex-calc record".into());
                        }
                        self.calcs.push(Calc { bone, vc: args[0], vs: args[1], vd: args[2], nc: args[3], ns: args[4], nd: args[5], hidx });
                    } else if (low & 0x18) != 0
                        && count != 0
                        && let Some(hdr) = &hdr
                    {
                        let base = hdr[0] as usize * 4 + *args.first().ok_or("primitive without data pointer")? as usize * 4;
                        for mut pr in decode_polys(d, low & 0xFF, count, base)? {
                            pr.bone = bone;
                            pr.shared = cat == 1;
                            // type bit 0x200000: semi-transparent
                            pr.semi = low & 0x20_0000 != 0;
                            self.polys.push((pr, hidx));
                        }
                    }
                }
                p = if nxt != 0xFFFF_FFFF { nxt & 0x7FFF_FFFF } else { 0x7FFF_FFFF };
            }
        }
        Ok(())
    }

    /// The s16 vector `idx` of the section at word `ptr`.
    pub fn svec(&self, ptr: u32, idx: u32) -> Result<[i64; 3], String> {
        let a = ptr as usize * 4 + idx as usize * 8;
        Ok([h16(self.d, a)? as i16 as i64, h16(self.d, a + 2)? as i16 as i64, h16(self.d, a + 4)? as i16 as i64])
    }
}
