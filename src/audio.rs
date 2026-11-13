//! Sound: VAB banks to WAV, the XA music and FMV audio streams to `.xa` files (the disc's CD-XA ADPCM as it is, format
//! below), and the XA decoder for the viewer's MP3 previews.
//!
//! `.xa`, version 1: `b"BR2XA\0"`, u8 version 1, u8 coding (the XA subheader's
//! coding-info byte: bit 0 stereo, bit 2 18.9 kHz else 37.8 kHz, bit 4 8-bit else 4-bit ADPCM), u32 LE sector count
//! N, u32 0; then N × 2304 bytes: each audio sector's 18 sound groups of 128 bytes (Mode 2 Form 2 user data without
//! its last 20 bytes), in disc order, up to and including the sector with the end-of-file submode bit.

pub const XA_MAGIC: &[u8; 6] = b"BR2XA\0";
pub const XA_VERSION: u8 = 1;
pub const XA_HEADER: usize = 16;
pub const XA_PAYLOAD: usize = 18 * 128;

const XA_K: [(i32, i32); 4] = [(0, 0), (60, 0), (115, -52), (98, -55)];
const SPU_K: [(i32, i32); 5] = [(0, 0), (60, 0), (115, -52), (98, -55), (122, -60)];

fn clamp(v: i32) -> i32 {
    v.clamp(-32768, 32767)
}

/// An `.xa` stream being collected sector by sector.
pub struct XaWriter {
    pub coding: u8,
    pub sectors: u32,
    pub data: Vec<u8>,
    pub done: bool,
}

impl XaWriter {
    pub fn new(coding: u8) -> XaWriter {
        let mut data = Vec::new();
        data.extend_from_slice(XA_MAGIC);
        data.push(XA_VERSION);
        data.push(coding);
        data.extend_from_slice(&[0; 8]);
        XaWriter { coding, sectors: 0, data, done: false }
    }
    /// Adds one raw 2352-byte audio sector (its 2304 sound bytes after the 24-byte header); returns true at the
    /// end-of-file sector.
    pub fn push(&mut self, raw: &[u8]) -> bool {
        self.data.extend_from_slice(&raw[24..24 + XA_PAYLOAD]);
        self.sectors += 1;
        let eof = raw[18] & 0x80 != 0;
        if eof {
            self.done = true;
        }
        eof
    }
    pub fn finish(mut self) -> Vec<u8> {
        let n = self.sectors.to_le_bytes();
        self.data[8..12].copy_from_slice(&n);
        self.data
    }
}

/// Decodes 4-bit stereo XA ADPCM sound groups (`audio._xa_sector`) into interleaved 16-bit PCM.
pub fn decode_xa(payload: &[u8], out: &mut Vec<i16>) {
    let mut state = [(0i32, 0i32); 2];
    let mut l = Vec::new();
    let mut r = Vec::new();
    for sector in payload.as_chunks::<XA_PAYLOAD>().0 {
        for grp in sector.as_chunks::<128>().0 {
            for unit in 0..8 {
                let param = grp[4 + unit];
                let shift = (param & 0xF) as i32;
                let (k0, k1) = XA_K[((param >> 4) & 3) as usize];
                let ch = unit & 1;
                let (mut s1, mut s2) = state[ch];
                let dst = if ch == 1 { &mut r } else { &mut l };
                let col = unit >> 1;
                for j in 0..28 {
                    let b = grp[16 + j * 4 + col];
                    let mut n = if ch == 1 { (b >> 4) as i32 } else { (b & 0xF) as i32 };
                    if n >= 8 {
                        n -= 16;
                    }
                    let v = if shift <= 12 { (n << 12) >> shift } else { 0 };
                    let v = clamp(v + ((s1 * k0 + s2 * k1 + 32) >> 6));
                    dst.push(v as i16);
                    s2 = s1;
                    s1 = v;
                }
                state[ch] = (s1, s2);
            }
        }
    }
    out.reserve(2 * l.len());
    for (a, b) in l.iter().zip(&r) {
        out.push(*a);
        out.push(*b);
    }
}

/// SPU ADPCM (VAG data without header) to 16-bit PCM (`audio.decode_spu_adpcm`).
pub fn decode_spu_adpcm(data: &[u8]) -> Vec<i16> {
    let mut out = Vec::new();
    let (mut s1, mut s2) = (0i32, 0i32);
    let mut b = 0;
    while data.len() >= 16 && b <= data.len() - 16 {
        let shift = (data[b] & 0xF) as i32;
        let f = ((data[b] >> 4) & 7) as usize;
        let (k0, k1) = SPU_K[if f < 5 { f } else { 0 }];
        let flags = data[b + 1];
        for i in 0..14 {
            let byte = data[b + 2 + i];
            for mut n in [(byte & 0xF) as i32, (byte >> 4) as i32] {
                if n >= 8 {
                    n -= 16;
                }
                let v = if shift <= 12 { (n << 12) >> shift } else { 0 };
                let v = clamp(v + ((s1 * k0 + s2 * k1 + 32) >> 6));
                out.push(v as i16);
                s2 = s1;
                s1 = v;
            }
        }
        if flags & 1 != 0 && b > 0 {
            break;
        }
        b += 16;
    }
    out
}

/// Every sample of a VAB bank: (VAG index, sample rate, PCM) (`audio.read_vab`).
pub fn read_vab(data: &[u8]) -> Vec<(usize, u32, Vec<i16>)> {
    if data.len() < 0x20 || &data[..4] != b"pBAV" {
        return Vec::new();
    }
    let u16_at = |o: usize| u16::from_le_bytes([data[o], data[o + 1]]) as usize;
    let (ps, vs) = (u16_at(0x12), u16_at(0x16));
    let tone_off = 0x20 + 128 * 16;
    let vag_tab = tone_off + ps * 16 * 32;
    let mut rates: std::collections::HashMap<usize, u32> = Default::default();
    for t in 0..ps * 16 {
        let o = tone_off + t * 32;
        let (center, fine, key) = (data[o + 4] as f64, data[o + 5] as f64, data[o + 6] as f64);
        let vag = u16_at(o + 22);
        if vag != 0 && !rates.contains_key(&vag) {
            // each tone is one key played at that key: rate = 44.1k * 2^((key - centre) / 12)
            let r = 44100.0 * libm::pow(2.0, (key - center + fine / 128.0) / 12.0);
            rates.insert(vag, crate::py::round_int(r) as u32);
        }
    }
    let mut pos = vag_tab + 512;
    let mut out = Vec::new();
    for i in 1..=vs {
        let sz = u16_at(vag_tab + 2 * i) * 8;
        let chunk = &data[pos.min(data.len())..(pos + sz).min(data.len())];
        pos += sz;
        if sz != 0 {
            out.push((i, rates.get(&i).copied().unwrap_or(22050), decode_spu_adpcm(chunk)));
        }
    }
    out
}

/// A 16-bit PCM WAV file (what Python's `wave` module writes).
pub fn wav(pcm: &[i16], rate: u32, channels: u16) -> Vec<u8> {
    let n = pcm.len() * 2;
    let mut o = Vec::with_capacity(44 + n);
    o.extend_from_slice(b"RIFF");
    o.extend_from_slice(&(36 + n as u32).to_le_bytes());
    o.extend_from_slice(b"WAVEfmt ");
    o.extend_from_slice(&16u32.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&channels.to_le_bytes());
    o.extend_from_slice(&rate.to_le_bytes());
    o.extend_from_slice(&(rate * 2 * channels as u32).to_le_bytes());
    o.extend_from_slice(&(2 * channels).to_le_bytes());
    o.extend_from_slice(&16u16.to_le_bytes());
    o.extend_from_slice(b"data");
    o.extend_from_slice(&(n as u32).to_le_bytes());
    for s in pcm {
        o.extend_from_slice(&s.to_le_bytes());
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_sector(chan: u8, submode: u8, fill: u8) -> Vec<u8> {
        let mut s = vec![0u8; 2352];
        s[17] = chan;
        s[18] = submode;
        s[19] = 1;
        for b in &mut s[24..24 + XA_PAYLOAD] {
            *b = fill;
        }
        s
    }

    #[test]
    fn xa_file_layout() {
        let mut w = XaWriter::new(1);
        assert!(!w.push(&raw_sector(0, 0x64, 7)));
        assert!(w.push(&raw_sector(0, 0xE4, 9)));
        let f = w.finish();
        assert_eq!(&f[..6], XA_MAGIC);
        assert_eq!((f[6], f[7]), (1, 1));
        assert_eq!(u32::from_le_bytes(f[8..12].try_into().unwrap()), 2);
        assert_eq!(f.len(), XA_HEADER + 2 * XA_PAYLOAD);
        assert_eq!((f[XA_HEADER], f[XA_HEADER + XA_PAYLOAD]), (7, 9));
    }

    #[test]
    fn xa_decoder_shapes() {
        // filter 0, shift 12: a nibble n decodes to n
        let mut sec = vec![0u8; XA_PAYLOAD];
        for g in sec.as_chunks_mut::<128>().0 {
            for u in 0..8 {
                g[4 + u] = 12;
            }
            for j in 0..28 {
                g[16 + j * 4] = 0x31; // unit 0 (left) = 1, unit 1 (right) = 3
            }
        }
        let mut pcm = Vec::new();
        decode_xa(&sec, &mut pcm);
        assert_eq!(pcm.len(), 2 * 18 * 4 * 28);
        assert_eq!(&pcm[..4], &[1, 3, 1, 3]);
    }

    #[test]
    fn spu_adpcm_and_wav() {
        let mut d = vec![0u8; 32];
        d[0] = 12;
        d[2] = 0x21;
        d[17] = 1;
        let p = decode_spu_adpcm(&d);
        assert_eq!(p.len(), 56);
        assert_eq!(&p[..2], &[1, 2]);
        let w = wav(&p, 11025, 1);
        assert_eq!(w.len(), 44 + 112);
        assert_eq!(&w[..4], b"RIFF");
    }
}
