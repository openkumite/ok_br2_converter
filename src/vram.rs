//! A minimal PS1 VRAM: TIM uploads and texture-page decoding.
//!
//! Textures are TIMs (magic 0x10) or split variants: 0x11 = CLUT block only, 0x12 = pixel block only. All share
//! the TIM block layout `u32 length, u16 x, u16 y, u16 w, u16 h`, then `w * h` halfwords.

pub const W: usize = 1024;
pub const H: usize = 512;

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

#[derive(Clone)]
pub struct Vram {
    pub mem: Vec<u16>,
    pub touched: Vec<bool>,
}

impl Default for Vram {
    fn default() -> Self {
        Vram { mem: vec![0; W * H], touched: vec![false; W * H] }
    }
}

impl Vram {
    fn upload(&mut self, x: usize, y: usize, w: usize, h: usize, data: &[u8], off: usize) {
        for r in 0..h {
            let yy = y + r;
            if yy >= H {
                break;
            }
            for c in 0..w {
                let xx = x + c;
                if xx < W {
                    self.mem[yy * W + xx] = u16_at(data, off + 2 * (r * w + c));
                    self.touched[yy * W + xx] = true;
                }
            }
        }
    }

    /// Uploads a TIM / CLUT-only / pixels-only entry; `at` replaces the stored position of a single-block entry
    /// (the game places character / stage textures by container slot). Returns false if not recognised.
    pub fn load_tim(&mut self, data: &[u8], at: Option<(usize, usize)>) -> bool {
        if data.len() < 20 {
            return false;
        }
        let (magic, flags) = (u32_at(data, 0), u32_at(data, 4));
        if !(0x10..=0x12).contains(&magic) {
            return false;
        }
        let mut p = 8;
        let blocks = if magic == 0x10 && flags & 8 != 0 { 2 } else { 1 };
        for _ in 0..blocks {
            if p + 12 > data.len() {
                return false;
            }
            let ln = u32_at(data, p) as usize;
            let (mut x, mut y, w, h) =
                (u16_at(data, p + 4) as usize, u16_at(data, p + 6) as usize, u16_at(data, p + 8) as usize, u16_at(data, p + 10) as usize);
            if p + 12 + w * h * 2 > data.len() {
                return false;
            }
            if let (Some(a), 1) = (at, blocks) {
                (x, y) = a;
            }
            self.upload(x, y, w, h, data, p + 12);
            p += ln;
        }
        true
    }

    /// A 256x256 texture page as RGBA (colour 0 = transparent).
    pub fn page_rgba(&self, tpage: u32, clut: u32) -> Vec<u8> {
        let px = (tpage as usize & 0xF) * 64;
        let py = ((tpage as usize >> 4) & 1) * 256;
        let mode = (tpage >> 7) & 3;
        let cx = (clut as usize & 0x3F) * 16;
        let cy = (clut as usize >> 6) & 0x1FF;
        let ncol = if mode == 0 { 16 } else { 256 };
        let pal: Vec<[u8; 4]> = if mode < 2 { (0..ncol).map(|i| rgba(self.mem[cy * W + ((cx + i) % W)])).collect() } else { Vec::new() };
        let mut out = vec![0u8; 256 * 256 * 4];
        let mut o = 0;
        for v in 0..256 {
            let row = ((py + v) % H) * W;
            for u in 0..256usize {
                let c = match mode {
                    0 => pal[((self.mem[row + (px + (u >> 2)) % W] >> ((u & 3) * 4)) & 0xF) as usize],
                    1 => pal[((self.mem[row + (px + (u >> 1)) % W] >> ((u & 1) * 8)) & 0xFF) as usize],
                    _ => rgba(self.mem[row + (px + u) % W]),
                };
                out[o..o + 4].copy_from_slice(&c);
                o += 4;
            }
        }
        out
    }
}

/// A PS1 colour as RGBA (0 = transparent black).
pub fn rgba(c: u16) -> [u8; 4] {
    if c == 0 {
        return [0; 4];
    }
    let r = ((c & 31) << 3) as u8;
    let g = (((c >> 5) & 31) << 3) as u8;
    let b = (((c >> 10) & 31) << 3) as u8;
    [r | r >> 5, g | g >> 5, b | b >> 5, 255]
}

/// Join four RGBA tiles (TL, TR, BL, BR) into one 2w x 2h image.
pub fn stitch_2x2(tiles: &[&[u8]; 4], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 * w * h);
    for row in 0..2 * h {
        let top = row < h;
        let r = if top { row } else { row - h };
        let (a, b) = if top { (tiles[0], tiles[1]) } else { (tiles[2], tiles[3]) };
        out.extend_from_slice(&a[r * w * 4..(r + 1) * w * 4]);
        out.extend_from_slice(&b[r * w * 4..(r + 1) * w * 4]);
    }
    out
}

/// A standalone 0x10 TIM as (width, height, RGBA), every pixel opaque (pictures are not cut out). None when not a
/// TIM, or where the data is too short.
pub fn tim_to_rgba(data: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    if data.len() < 8 || u32_at(data, 0) != 0x10 {
        return None;
    }
    let flags = u32_at(data, 4);
    let mut p = 8;
    let mut clut: Vec<[u8; 4]> = Vec::new();
    if flags & 8 != 0 {
        if p + 12 > data.len() {
            return None;
        }
        let ln = u32_at(data, p) as usize;
        let (w, h) = (u16_at(data, p + 8) as usize, u16_at(data, p + 10) as usize);
        if p + 12 + 2 * w * h > data.len() {
            return None;
        }
        clut = (0..w * h).map(|i| rgba(u16_at(data, p + 12 + 2 * i))).collect();
        p += ln;
    }
    if p + 12 > data.len() {
        return None;
    }
    let (w, h) = (u16_at(data, p + 8) as usize, u16_at(data, p + 10) as usize);
    let pix = &data[(p + 12).min(data.len())..(p + 12 + w * h * 2).min(data.len())];
    let bpp = flags & 3;
    let mut out = Vec::new();
    let width;
    let look = |i: usize| -> Option<[u8; 4]> { clut.get(i).copied() };
    if bpp == 0 {
        width = w * 4;
        for &b in pix {
            out.extend_from_slice(&look((b & 15) as usize)?);
            out.extend_from_slice(&look((b >> 4) as usize)?);
        }
    } else if bpp == 1 {
        width = w * 2;
        for &b in pix {
            out.extend_from_slice(&look(b as usize)?);
        }
    } else {
        width = w;
        if pix.len() < w * h * 2 {
            return None;
        }
        for i in 0..w * h {
            out.extend_from_slice(&rgba(u16_at(pix, 2 * i)));
        }
    }
    for i in (3..out.len()).step_by(4) {
        out[i] = 255;
    }
    Some((width, h, out))
}

/// A 4-bit page (TIM block, no palette) as grayscale; the index is the brightness (story text, portraits).
/// `crop`: rows after the last non-empty one + 5 are cut.
pub fn page_gray4(data: &[u8], crop: bool) -> (usize, usize, Vec<u8>) {
    let (w, mut h) = (u16_at(data, 16) as usize, u16_at(data, 18) as usize);
    let px = &data[20.min(data.len())..(20 + w * h * 2).min(data.len())];
    let width = w * 4;
    let mut vals: Vec<u8> = Vec::with_capacity(px.len() * 2);
    for &b in px {
        vals.push((b & 15) * 17);
        vals.push((b >> 4) * 17);
    }
    if crop {
        let mut last = 0;
        for r in 0..h {
            let row = &vals[(r * width).min(vals.len())..((r + 1) * width).min(vals.len())];
            if row.iter().any(|&v| v != 0) {
                last = r;
            }
        }
        h = h.min(last + 6);
        vals.truncate(width * h);
    }
    let mut out = vec![0u8; vals.len() * 4];
    for (i, &v) in vals.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&[v, v, v, 255]);
    }
    (width, h, out)
}

/// An 8-bit page (TIM block) with a 256-colour palette (colour 0 = opaque black here), or grayscale.
pub fn page_8bit(data: &[u8], palette: Option<&[u16]>) -> (usize, usize, Vec<u8>) {
    let (w, h) = (u16_at(data, 16) as usize, u16_at(data, 18) as usize);
    let px = &data[20.min(data.len())..(20 + w * h * 2).min(data.len())];
    let width = w * 2;
    let mut out = Vec::with_capacity(px.len() * 4);
    match palette {
        Some(pal) if !pal.is_empty() => {
            let lut: Vec<[u8; 4]> = pal.iter().map(|&c| if c != 0 { rgba(c) } else { [0, 0, 0, 255] }).collect();
            for &b in px {
                out.extend_from_slice(&lut[b as usize]);
            }
        }
        _ => {
            for &b in px {
                out.extend_from_slice(&[b, b, b, 255]);
            }
        }
    }
    (width, h, out)
}
