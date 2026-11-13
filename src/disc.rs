//! Disc images: BIN/CUE (raw 2352-byte sectors), CHD (MAME, feature `chd`) and ISO (2048-byte sectors), the
//! ISO 9660 file system on them and the check that it is the right game.
//!
//! A raw image keeps every sector whole (sync, header, the CD-XA subheader, the Mode 2 Form 2 audio / video of the
//! XA and STR streams); an ISO holds only the 2048 user bytes of each sector, so the streams cannot be read from it.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};

pub const RAW_SECTOR: usize = 2352;
pub const ISO_SECTOR: usize = 2048;
const SYNC: [u8; 12] = [0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0];

/// The game this converter is for: the ISO 9660 volume id and the boot file of SYSTEM.CNF.
pub const VOLUME_ID: &str = "SCUS_94424";
pub const BOOT_EXE: &str = "SCUS_944.24";

#[derive(Debug)]
pub enum DiscError {
    Io(String),
    Format(String),
    /// A readable disc, but not Bloody Roar II (USA); `found` says what it is.
    WrongDisc {
        found: String,
    },
    /// Raw 2352-byte sectors needed (a CUE track of another mode; the XA / STR streams of an ISO).
    NotRaw2352(String),
}

impl std::fmt::Display for DiscError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            DiscError::Io(e) => write!(f, "read error: {e}"),
            DiscError::Format(e) => write!(f, "not a usable disc image: {e}"),
            DiscError::WrongDisc { found } => write!(f, "this is not Bloody Roar II (USA, SCUS-94424): {found}"),
            DiscError::NotRaw2352(e) => write!(f, "raw 2352-byte sectors needed: {e}"),
        }
    }
}

impl std::error::Error for DiscError {}

impl From<std::io::Error> for DiscError {
    fn from(e: std::io::Error) -> DiscError {
        DiscError::Io(e.to_string())
    }
}

/// `Send` where threads exist (a conversion can run on a background thread), nothing on wasm32 (a reader there
/// may hold JavaScript objects).
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

/// A CD-DA (audio) track of an image: its number and its length in 2352-byte sectors from its INDEX 01.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioTrack {
    pub number: u32,
    pub sectors: u32,
}

/// Where the sectors come from.
pub trait SectorReader: MaybeSend {
    /// True when `read_raw` gives whole 2352-byte sectors (BIN, CHD); false for an ISO.
    fn raw(&self) -> bool;
    /// Number of sectors of the data track.
    fn sectors(&self) -> u32;
    /// Raw images: the whole 2352-byte sector `lba` into `out[..2352]`. ISO: the 2048 user bytes into `out[..2048]`.
    fn read_sector(&mut self, lba: u32, out: &mut [u8]) -> Result<(), DiscError>;
    /// What the image is, for manifest.json `source`: "bin" (raw sectors), "iso" or "chd".
    fn source(&self) -> &'static str {
        if self.raw() { "bin" } else { "iso" }
    }
    /// The image's CD-DA tracks (a CUE opened with `open_cue_all`, a CHD); none by default.
    fn audio_tracks(&self) -> Vec<AudioTrack> {
        Vec::new()
    }
    /// Sector `sector` (counted from INDEX 01) of CD-DA track `track`: 588 stereo frames of 16-bit little-endian PCM
    /// into `out[..2352]`.
    fn read_audio(&mut self, track: u32, sector: u32, out: &mut [u8]) -> Result<(), DiscError> {
        let _ = (sector, out);
        Err(DiscError::Format(format!("the image has no CD-DA track {track}")))
    }
}

/// A BIN (raw 2352-byte sectors) or ISO (2048) file read through `Read + Seek`; `offset` = the byte where the data
/// track starts (the CUE's INDEX 01 of track 1, normally 0).
pub struct FileImage<R> {
    r: R,
    sector: usize,
    offset: u64,
    count: u32,
}

impl<R: Read + Seek + MaybeSend> FileImage<R> {
    pub fn raw(mut r: R, offset: u64) -> Result<Self, DiscError> {
        let len = r.seek(SeekFrom::End(0))?;
        Ok(FileImage { r, sector: RAW_SECTOR, offset, count: (len.saturating_sub(offset) / RAW_SECTOR as u64) as u32 })
    }
    pub fn iso(mut r: R) -> Result<Self, DiscError> {
        let len = r.seek(SeekFrom::End(0))?;
        Ok(FileImage { r, sector: ISO_SECTOR, offset: 0, count: (len / ISO_SECTOR as u64) as u32 })
    }
}

impl<R: Read + Seek + MaybeSend> SectorReader for FileImage<R> {
    fn raw(&self) -> bool {
        self.sector == RAW_SECTOR
    }
    fn sectors(&self) -> u32 {
        self.count
    }
    fn read_sector(&mut self, lba: u32, out: &mut [u8]) -> Result<(), DiscError> {
        if lba >= self.count {
            return Err(DiscError::Format(format!("sector {lba} past the end of the image ({} sectors)", self.count)));
        }
        self.r.seek(SeekFrom::Start(self.offset + lba as u64 * self.sector as u64))?;
        self.r.read_exact(&mut out[..self.sector])?;
        Ok(())
    }
}

/// Random-access input: a native File, an Android fd, a browser Blob read with FileReaderSync in a worker.
pub trait ReadAt: MaybeSend {
    /// Fills `buf` from byte `offset` (an error if the input ends first).
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<()>;
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// `Read + Seek` over a `ReadAt`.
pub struct ReadAtStream {
    inner: Box<dyn ReadAt>,
    pos: u64,
}

impl ReadAtStream {
    pub fn new(inner: Box<dyn ReadAt>) -> Self {
        ReadAtStream { inner, pos: 0 }
    }
}

impl Read for ReadAtStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = (self.inner.len().saturating_sub(self.pos)).min(buf.len() as u64) as usize;
        if n > 0 {
            self.inner.read_at(self.pos, &mut buf[..n])?;
            self.pos += n as u64;
        }
        Ok(n)
    }
}

impl Seek for ReadAtStream {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.inner.len() as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if p < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

/// Opens an image given as `ReadAt` (CHD, raw BIN, ISO: by contents).
pub fn open_read_at(input: Box<dyn ReadAt>) -> Result<Box<dyn SectorReader>, DiscError> {
    open_image(ReadAtStream::new(input))
}

/// Opens the data track of a CUE sheet; `open` gives the file a FILE line names (no file system in the core).
pub fn open_cue(cue_text: &str, open: &mut dyn FnMut(&str) -> std::io::Result<Box<dyn ReadAt>>) -> Result<Box<dyn SectorReader>, DiscError> {
    let (file, offset) = cue_data_track(cue_text)?;
    let r = open(&file).map_err(|e| DiscError::Io(format!("{file}: {e}")))?;
    Ok(Box::new(FileImage::raw(ReadAtStream::new(r), offset)?))
}

/// A CUE sheet's data track with its CD-DA tracks (`open_cue_all`).
pub struct CueImage {
    data: Box<dyn SectorReader>,
    /// each audio track, its file and the byte where its INDEX 01 starts
    audio: Vec<(AudioTrack, Box<dyn ReadAt>, u64)>,
    /// the FILEs of audio tracks that could not be opened (the tracks are left out)
    pub missing: Vec<String>,
}

impl SectorReader for CueImage {
    fn raw(&self) -> bool {
        self.data.raw()
    }
    fn sectors(&self) -> u32 {
        self.data.sectors()
    }
    fn read_sector(&mut self, lba: u32, out: &mut [u8]) -> Result<(), DiscError> {
        self.data.read_sector(lba, out)
    }
    fn audio_tracks(&self) -> Vec<AudioTrack> {
        self.audio.iter().map(|a| a.0).collect()
    }
    fn read_audio(&mut self, track: u32, sector: u32, out: &mut [u8]) -> Result<(), DiscError> {
        let (t, f, at) = self.audio.iter_mut().find(|a| a.0.number == track).ok_or_else(|| DiscError::Format(format!("no CD-DA track {track}")))?;
        if sector >= t.sectors {
            return Err(DiscError::Format(format!("sector {sector} past the end of track {track}")));
        }
        f.read_at(*at + sector as u64 * RAW_SECTOR as u64, &mut out[..RAW_SECTOR]).map_err(DiscError::from)
    }
}

/// Opens a CUE sheet's data track (as `open_cue`) and its AUDIO tracks, each from its FILE's INDEX 01 to the next
/// track of that file (its INDEX 00 if it has one) or the file's end. An audio FILE that cannot be opened is left out
/// (`CueImage::missing` names it): the data track is what a conversion needs.
pub fn open_cue_all(cue_text: &str, open: &mut dyn FnMut(&str) -> std::io::Result<Box<dyn ReadAt>>) -> Result<CueImage, DiscError> {
    let tracks = parse_cue(cue_text)?;
    let data = open_cue(cue_text, open)?;
    let (mut audio, mut missing) = (Vec::new(), Vec::new());
    for (k, t) in tracks.iter().enumerate().filter(|(_, t)| t.mode == "AUDIO") {
        let f = match open(&t.file) {
            Ok(f) => f,
            Err(_) => {
                if !missing.contains(&t.file) {
                    missing.push(t.file.clone());
                }
                continue;
            }
        };
        let file_sectors = (f.len() / RAW_SECTOR as u64) as u32;
        let end = tracks[k + 1..].iter().find(|n| n.file == t.file).map_or(file_sectors, |n| n.index0.unwrap_or(n.start)).min(file_sectors);
        let sectors = end.saturating_sub(t.start);
        audio.push((AudioTrack { number: t.number, sectors }, f, t.start as u64 * RAW_SECTOR as u64));
    }
    Ok(CueImage { data, audio, missing })
}

/// An image held in memory (the web build reads the picked file whole).
pub fn memory_image(bytes: Vec<u8>) -> Result<Box<dyn SectorReader>, DiscError> {
    open_image(std::io::Cursor::new(bytes))
}

/// Opens an image by its contents: CHD (`MComprHD`), raw 2352-byte sectors (sync pattern at 0), else ISO 9660 with
/// 2048-byte sectors ("CD001" at sector 16).
pub fn open_image<R: Read + Seek + MaybeSend + 'static>(mut r: R) -> Result<Box<dyn SectorReader>, DiscError> {
    let mut head = [0u8; 16];
    r.seek(SeekFrom::Start(0))?;
    let n = read_up_to(&mut r, &mut head)?;
    if n >= 8 && &head[..8] == b"MComprHD" {
        #[cfg(feature = "chd")]
        return Ok(Box::new(crate::chd_image::ChdImage::open(r)?));
        #[cfg(not(feature = "chd"))]
        return Err(DiscError::Format("CHD images need the converter's `chd` feature".into()));
    }
    if n >= 12 && head[..12] == SYNC {
        return Ok(Box::new(FileImage::raw(r, 0)?));
    }
    let mut pvd = [0u8; 6];
    r.seek(SeekFrom::Start(16 * ISO_SECTOR as u64))?;
    if read_up_to(&mut r, &mut pvd)? == 6 && &pvd[1..6] == b"CD001" {
        return Ok(Box::new(FileImage::iso(r)?));
    }
    Err(DiscError::Format("neither a CHD, a raw (2352-byte sector) BIN nor an ISO 9660 image".into()))
}

fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<usize, DiscError> {
    let mut n = 0;
    while n < buf.len() {
        let k = r.read(&mut buf[n..])?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

/// One track of a CUE sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct CueTrack {
    pub number: u32,
    /// "MODE2/2352", "MODE1/2048", "AUDIO", …
    pub mode: String,
    /// The FILE this track is in.
    pub file: String,
    /// INDEX 01 as a sector (frame) offset into that file.
    pub start: u32,
    /// INDEX 00 (the start of the pregap), if the sheet gives one.
    pub index0: Option<u32>,
}

/// Parses a CUE sheet: its FILE / TRACK / INDEX 01 lines.
pub fn parse_cue(text: &str) -> Result<Vec<CueTrack>, DiscError> {
    let mut tracks: Vec<CueTrack> = Vec::new();
    let mut file: Option<String> = None;
    for line in text.lines() {
        let l = line.trim();
        let (kw, rest) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
        match kw.to_ascii_uppercase().as_str() {
            "FILE" => {
                let rest = rest.trim();
                let name = if let Some(r) = rest.strip_prefix('"') {
                    r.split('"').next().unwrap_or("").to_string()
                } else {
                    rest.split_whitespace().next().unwrap_or("").to_string()
                };
                file = Some(name);
            }
            "TRACK" => {
                let mut it = rest.split_whitespace();
                let number = it.next().and_then(|n| n.parse().ok()).ok_or_else(|| DiscError::Format(format!("bad CUE line: {l}")))?;
                let mode = it.next().unwrap_or("").to_ascii_uppercase();
                let f = file.clone().ok_or_else(|| DiscError::Format("CUE TRACK before any FILE".into()))?;
                tracks.push(CueTrack { number, mode, file: f, start: 0, index0: None });
            }
            "INDEX" => {
                let mut it = rest.split_whitespace();
                let n = it.next().and_then(|n| n.parse::<u32>().ok());
                if n == Some(1) || n == Some(0) {
                    let msf = it.next().unwrap_or("");
                    let p: Vec<u32> = msf.split(':').filter_map(|x| x.parse().ok()).collect();
                    if p.len() != 3 {
                        return Err(DiscError::Format(format!("bad CUE index: {l}")));
                    }
                    let at = (p[0] * 60 + p[1]) * 75 + p[2];
                    if let Some(t) = tracks.last_mut() {
                        if n == Some(1) {
                            t.start = at;
                        } else {
                            t.index0 = Some(at);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if tracks.is_empty() {
        return Err(DiscError::Format("the CUE sheet has no tracks".into()));
    }
    Ok(tracks)
}

/// The data track of a CUE sheet as the converter needs it: track 1, MODE2/2352 (or MODE1/2352). Returns the
/// FILE name and the byte offset of the track in it.
pub fn cue_data_track(text: &str) -> Result<(String, u64), DiscError> {
    let tracks = parse_cue(text)?;
    let t = &tracks[0];
    if t.number != 1 || !(t.mode == "MODE2/2352" || t.mode == "MODE1/2352") {
        return Err(DiscError::NotRaw2352(format!("track 1 is {} {}, not a raw MODE2/2352 data track", t.number, t.mode)));
    }
    Ok((t.file.clone(), t.start as u64 * RAW_SECTOR as u64))
}

/// A file on the disc: its first sector and its size in bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    pub lba: u32,
    pub size: u32,
}

/// The disc: its sectors and the ISO 9660 directory tree.
pub struct Disc {
    src: Box<dyn SectorReader>,
    /// `/DIR/NAME.EXT` → entry, in directory order.
    pub order: Vec<(String, Entry)>,
    pub files: BTreeMap<String, Entry>,
    buf: Vec<u8>,
}

impl Disc {
    /// Reads the file system and checks the game (volume id SCUS_94424, SYSTEM.CNF booting SCUS_944.24).
    pub fn open(src: Box<dyn SectorReader>) -> Result<Disc, DiscError> {
        let mut d = Disc { src, order: Vec::new(), files: BTreeMap::new(), buf: vec![0; RAW_SECTOR] };
        let pvd = d.sector(16)?;
        if &pvd[1..6] != b"CD001" {
            return Err(DiscError::Format("no ISO 9660 volume descriptor at sector 16".into()));
        }
        let system = String::from_utf8_lossy(&pvd[8..40]).trim().to_string();
        let volume = String::from_utf8_lossy(&pvd[40..72]).trim().to_string();
        if system != "PLAYSTATION" {
            return Err(DiscError::WrongDisc { found: format!("system id \"{system}\" (a PlayStation disc says PLAYSTATION)") });
        }
        if volume != VOLUME_ID {
            return Err(DiscError::WrongDisc { found: format!("volume id \"{volume}\", expected {VOLUME_ID}") });
        }
        let r = &pvd[156..190];
        let (lba, size) = (u32::from_le_bytes(r[2..6].try_into().unwrap()), u32::from_le_bytes(r[10..14].try_into().unwrap()));
        let mut seen = Vec::new();
        d.walk(lba, size, "", &mut seen)?;
        let cnf = d.read("/SYSTEM.CNF").map_err(|_| DiscError::WrongDisc { found: "no SYSTEM.CNF".into() })?;
        let cnf = String::from_utf8_lossy(&cnf).to_string();
        if !cnf.contains(BOOT_EXE) || !d.files.contains_key(&format!("/{BOOT_EXE}")) {
            return Err(DiscError::WrongDisc { found: format!("SYSTEM.CNF does not boot {BOOT_EXE}") });
        }
        Ok(d)
    }

    /// "bin", "chd" or "iso" (manifest.json `source`).
    pub fn source(&self) -> &'static str {
        self.src.source()
    }

    /// True for BIN/CUE and CHD (the XA / STR streams can be read), false for an ISO.
    pub fn raw(&self) -> bool {
        self.src.raw()
    }

    /// The 2048 user bytes of sector `lba` (Mode 2 Form 1: after the 24-byte sync, header and subheader).
    pub fn sector(&mut self, lba: u32) -> Result<Vec<u8>, DiscError> {
        let raw = self.src.raw();
        self.src.read_sector(lba, &mut self.buf)?;
        Ok(if raw { self.buf[24..24 + ISO_SECTOR].to_vec() } else { self.buf[..ISO_SECTOR].to_vec() })
    }

    /// The whole 2352-byte sector (raw images only).
    pub fn raw_sector(&mut self, lba: u32) -> Result<&[u8], DiscError> {
        if !self.src.raw() {
            return Err(DiscError::NotRaw2352("an ISO image has no raw sectors".into()));
        }
        self.src.read_sector(lba, &mut self.buf)?;
        Ok(&self.buf[..RAW_SECTOR])
    }

    fn read_span(&mut self, lba: u32, size: u32) -> Result<Vec<u8>, DiscError> {
        let n = (size as usize).div_ceil(ISO_SECTOR);
        let mut out = Vec::with_capacity(n * ISO_SECTOR);
        for i in 0..n {
            out.extend_from_slice(&self.sector(lba + i as u32)?);
        }
        out.truncate(size as usize);
        Ok(out)
    }

    fn walk(&mut self, lba: u32, size: u32, path: &str, seen: &mut Vec<u32>) -> Result<(), DiscError> {
        if seen.contains(&lba) || seen.len() > 4096 {
            return Err(DiscError::Format("looping directory tree".into()));
        }
        seen.push(lba);
        let data = self.read_span(lba, size)?;
        let mut i = 0;
        while i < data.len() {
            let ln = data[i] as usize;
            if ln == 0 {
                i = (i / ISO_SECTOR + 1) * ISO_SECTOR;
                continue;
            }
            let rec = &data[i..(i + ln).min(data.len())];
            if rec.len() < 34 {
                return Err(DiscError::Format("short directory record".into()));
            }
            let el = u32::from_le_bytes(rec[2..6].try_into().unwrap());
            let ds = u32::from_le_bytes(rec[10..14].try_into().unwrap());
            let nl = rec[32] as usize;
            let name = &rec[33..(33 + nl).min(rec.len())];
            if name != b"\0" && name != b"\x01" {
                let s = String::from_utf8_lossy(name);
                let n = s.split(';').next().unwrap_or("").trim_end_matches('.').to_string();
                let full = format!("{path}/{n}");
                if rec[25] & 2 != 0 {
                    self.walk(el, ds, &full, seen)?;
                } else {
                    let e = Entry { lba: el, size: ds };
                    if self.files.insert(full.clone(), e).is_some() {
                        self.order.retain(|x| x.0 != full);
                    }
                    self.order.push((full, e));
                }
            }
            i += ln;
        }
        Ok(())
    }

    pub fn entry(&self, name: &str) -> Option<Entry> {
        self.files.get(name).copied()
    }

    /// A file's bytes (as Form 1 sectors).
    pub fn read(&mut self, name: &str) -> Result<Vec<u8>, DiscError> {
        let e = self.entry(name).ok_or_else(|| DiscError::Format(format!("{name} is not on the disc")))?;
        self.read_span(e.lba, e.size)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn cue_sheet() {
        let cue = "FILE \"Bloody Roar II (USA).bin\" BINARY\r\n  TRACK 01 MODE2/2352\r\n    INDEX 01 00:00:00\r\n";
        let t = parse_cue(cue).unwrap();
        assert_eq!(t, vec![CueTrack { number: 1, mode: "MODE2/2352".into(), file: "Bloody Roar II (USA).bin".into(), start: 0, index0: None }]);
        assert_eq!(cue_data_track(cue).unwrap(), ("Bloody Roar II (USA).bin".to_string(), 0));
        let two = "FILE game.bin BINARY\nTRACK 01 MODE2/2352\nINDEX 01 00:02:05\nFILE b.bin BINARY\nTRACK 02 AUDIO\nINDEX 00 00:00:00\nINDEX 01 00:02:00\n";
        let t = parse_cue(two).unwrap();
        assert_eq!(t[0].start, 2 * 75 + 5);
        assert_eq!(t[1].file, "b.bin");
        assert_eq!((t[1].start, t[1].index0), (150, Some(0)));
        assert!(cue_data_track("FILE a.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n").is_err());
    }

    /// A tiny ISO 9660 image: PVD at 16, root directory at 18 with SYSTEM.CNF, SCUS_944.24 and a subdirectory.
    pub fn tiny_iso(raw: bool, volume: &str) -> Vec<u8> {
        let mut sectors = vec![vec![0u8; ISO_SECTOR]; 24];
        let p = &mut sectors[16];
        p[0] = 1;
        p[1..6].copy_from_slice(b"CD001");
        let pad = |s: &str| format!("{s:<32}").into_bytes();
        p[8..40].copy_from_slice(&pad("PLAYSTATION"));
        p[40..72].copy_from_slice(&pad(volume));
        fn rec(lba: u32, size: u32, dir: bool, name: &[u8]) -> Vec<u8> {
            let mut r = vec![0u8; 33];
            r[2..6].copy_from_slice(&lba.to_le_bytes());
            r[10..14].copy_from_slice(&size.to_le_bytes());
            r[25] = if dir { 2 } else { 0 };
            r[32] = name.len() as u8;
            r.extend_from_slice(name);
            if r.len() % 2 == 1 {
                r.push(0);
            }
            r[0] = r.len() as u8;
            r
        }
        let root = rec(18, 2048, true, b"\0");
        sectors[16][156..156 + root.len()].copy_from_slice(&root);
        let mut dir = Vec::new();
        dir.extend(rec(18, 2048, true, b"\0"));
        dir.extend(rec(18, 2048, true, b"\x01"));
        dir.extend(rec(20, 28, false, b"SYSTEM.CNF;1"));
        dir.extend(rec(21, 3000, false, b"SCUS_944.24;1"));
        dir.extend(rec(19, 2048, true, b"CMN"));
        sectors[18][..dir.len()].copy_from_slice(&dir);
        let mut sub = Vec::new();
        sub.extend(rec(19, 2048, true, b"\0"));
        sub.extend(rec(18, 2048, true, b"\x01"));
        sub.extend(rec(23, 3, false, b"A.DAT;1"));
        sectors[19][..sub.len()].copy_from_slice(&sub);
        sectors[20][..28].copy_from_slice(b"BOOT = cdrom:\\SCUS_944.24;1\n");
        sectors[21][0] = 0xAB;
        sectors[22][2999 - 2048] = 0xCD;
        sectors[23][..3].copy_from_slice(b"abc");
        let mut out = Vec::new();
        for (i, s) in sectors.iter().enumerate() {
            if raw {
                out.extend_from_slice(&SYNC);
                out.extend_from_slice(&[0, 2, i as u8, 2]);
                out.extend_from_slice(&[0, 0, 8, 0, 0, 0, 8, 0]);
                out.extend_from_slice(s);
                out.extend_from_slice(&[0u8; 280]);
            } else {
                out.extend_from_slice(s);
            }
        }
        out
    }

    struct Mem(Vec<u8>);
    impl ReadAt for Mem {
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
            let o = offset as usize;
            let src = self.0.get(o..o + buf.len()).ok_or(std::io::ErrorKind::UnexpectedEof)?;
            buf.copy_from_slice(src);
            Ok(())
        }
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
    }

    #[test]
    fn read_at_and_cue_opener() {
        let raw = tiny_iso(true, VOLUME_ID);
        let mut d = Disc::open(open_read_at(Box::new(Mem(raw.clone()))).unwrap()).unwrap();
        assert_eq!(d.read("/CMN/A.DAT").unwrap(), b"abc");
        let cue = "FILE \"game.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n";
        let mut asked = String::new();
        let img = open_cue(cue, &mut |f| {
            asked = f.to_string();
            Ok(Box::new(Mem(raw.clone())))
        })
        .unwrap();
        assert_eq!(asked, "game.bin");
        assert_eq!(img.source(), "bin");
        assert!(matches!(
            open_cue("FILE a.bin BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n", &mut |_| Ok(Box::new(Mem(Vec::new())))),
            Err(DiscError::NotRaw2352(_))
        ));
    }

    #[test]
    fn cue_with_audio_tracks() {
        // the data track in one file; track 2 (pregap 2 sectors) and track 3 in a second one; track 4's file missing
        let cue = "FILE \"d.bin\" BINARY\n TRACK 01 MODE2/2352\n  INDEX 01 00:00:00\nFILE \"a.bin\" BINARY\n TRACK 02 AUDIO\n  INDEX 00 00:00:00\n  INDEX 01 00:00:02\n TRACK 03 AUDIO\n  INDEX 00 00:00:05\n  INDEX 01 00:00:06\nFILE \"gone.bin\" BINARY\n TRACK 04 AUDIO\n  INDEX 01 00:00:00\n";
        let raw = tiny_iso(true, VOLUME_ID);
        let audio: Vec<u8> = (0..9 * RAW_SECTOR).map(|i| (i / RAW_SECTOR) as u8).collect();
        let mut img = open_cue_all(cue, &mut |f| match f {
            "d.bin" => Ok(Box::new(Mem(raw.clone()))),
            "a.bin" => Ok(Box::new(Mem(audio.clone()))),
            _ => Err(std::io::ErrorKind::NotFound.into()),
        })
        .unwrap();
        assert_eq!(img.missing, ["gone.bin"]);
        assert_eq!(img.audio_tracks(), [AudioTrack { number: 2, sectors: 3 }, AudioTrack { number: 3, sectors: 3 }]);
        let mut s = vec![0u8; RAW_SECTOR];
        img.read_audio(2, 0, &mut s).unwrap();
        assert_eq!(s[0], 2);
        img.read_audio(3, 2, &mut s).unwrap();
        assert_eq!(s[0], 8);
        assert!(img.read_audio(3, 3, &mut s).is_err() && img.read_audio(4, 0, &mut s).is_err());
        assert_eq!(Disc::open(Box::new(img)).unwrap().read("/CMN/A.DAT").unwrap(), b"abc");
    }

    #[test]
    fn iso9660_and_raw_sectors() {
        for raw in [false, true] {
            let img = memory_image(tiny_iso(raw, VOLUME_ID)).unwrap();
            assert_eq!(img.raw(), raw);
            let mut d = Disc::open(img).unwrap();
            let names: Vec<&str> = d.order.iter().map(|e| e.0.as_str()).collect();
            assert_eq!(names, ["/SYSTEM.CNF", "/SCUS_944.24", "/CMN/A.DAT"]);
            assert_eq!(d.read("/CMN/A.DAT").unwrap(), b"abc");
            let exe = d.read("/SCUS_944.24").unwrap();
            assert_eq!((exe.len(), exe[0], exe[2999]), (3000, 0xAB, 0xCD));
            if raw {
                let s = d.raw_sector(23).unwrap();
                assert_eq!(&s[..12], &SYNC);
                assert_eq!(&s[24..27], b"abc");
            } else {
                assert!(d.raw_sector(23).is_err());
            }
        }
        assert!(matches!(Disc::open(memory_image(tiny_iso(true, "SLUS_00000")).unwrap()), Err(DiscError::WrongDisc { found: _ })));
    }
}
