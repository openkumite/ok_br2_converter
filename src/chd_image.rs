//! CHD disc images (MAME's compressed hunks of data) through the `chd` crate: track 1 (the data track), frame by
//! frame, and the AUDIO tracks after it. A CD frame in a CHD is 2448 bytes (2352 of sector data, 96 of subcode);
//! MODE1 / MODE2_FORM1 tracks store only the 2048 user bytes at its start. The tracks follow each other, each padded to
//! a multiple of 4 frames (MAME's `CD_TRACK_PADDING`); a track whose pregap type starts with `V` holds its pregap's
//! frames before INDEX 01. Audio is stored with its 16-bit samples big-endian (byte-swapped back on reading).

use crate::disc::{AudioTrack, DiscError, ISO_SECTOR, RAW_SECTOR, SectorReader};
use std::io::{Read, Seek};

const FRAME: usize = 2448;

pub struct ChdImage<R: Read + Seek> {
    chd: chd::Chd<R>,
    raw: bool,
    frames: u32,
    frames_per_hunk: u32,
    hunk: Vec<u8>,
    hunk_no: Option<u32>,
    cmp: Vec<u8>,
    /// each AUDIO track and the CHD frame of its INDEX 01
    audio: Vec<(AudioTrack, u32)>,
}

fn err(e: chd::Error) -> DiscError {
    DiscError::Format(format!("CHD: {e}"))
}

impl<R: Read + Seek> ChdImage<R> {
    pub fn open(r: R) -> Result<Self, DiscError> {
        let mut chd = chd::Chd::open(r, None).map_err(err)?;
        let hunk_size = chd.header().hunk_size() as usize;
        if chd.header().unit_bytes() as usize != FRAME || !hunk_size.is_multiple_of(FRAME) {
            return Err(DiscError::Format("this CHD is not a CD image".into()));
        }
        let metas: Vec<chd::metadata::Metadata> = Vec::try_from(chd.metadata_refs()).map_err(err)?;
        // CHT2 / CHTR: "TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:235004 ..."
        let records: Vec<String> = metas
            .iter()
            .filter(|m| &m.metatag.to_be_bytes() == b"CHT2" || &m.metatag.to_be_bytes() == b"CHTR")
            .map(|m| String::from_utf8_lossy(&m.value).trim_end_matches('\0').to_string())
            .collect();
        let track1 =
            records.iter().find(|t| t.split_whitespace().any(|w| w == "TRACK:1")).ok_or_else(|| DiscError::Format("the CHD has no CD track 1".into()))?;
        let field = |k: &str| track1.split_whitespace().find_map(|w| w.strip_prefix(k)).unwrap_or("").to_string();
        let ty = field("TYPE:");
        let raw = match ty.as_str() {
            "MODE2_RAW" | "MODE1_RAW" => true,
            "MODE1" | "MODE2_FORM1" => false,
            other => return Err(DiscError::Format(format!("CHD track 1 is {other}, not a data track"))),
        };
        let frames: u32 = field("FRAMES:").parse().map_err(|_| DiscError::Format("CHD track 1 without FRAMES".into()))?;
        let hunk = chd.get_hunksized_buffer();
        let frames_per_hunk = (hunk_size / FRAME) as u32;
        // the frames the CHD holds: an AUDIO record's FRAMES past them is not believed
        let stored = u64::from(chd.header().hunk_count()) * u64::from(frames_per_hunk);
        let audio = audio_tracks(&records, stored.min(u64::from(u32::MAX - 3)) as u32);
        Ok(ChdImage { chd, raw, frames, frames_per_hunk, hunk, hunk_no: None, cmp: Vec::new(), audio })
    }

    /// Frame `frame` of the CHD (2448 bytes) through the hunk cache.
    fn frame(&mut self, frame: u32) -> Result<&[u8], DiscError> {
        let h = frame / self.frames_per_hunk;
        if self.hunk_no != Some(h) {
            self.hunk_no = None;
            let mut hunk = self.chd.hunk(h).map_err(err)?;
            hunk.read_hunk_in(&mut self.cmp, &mut self.hunk).map_err(err)?;
            self.hunk_no = Some(h);
        }
        let o = (frame % self.frames_per_hunk) as usize * FRAME;
        Ok(&self.hunk[o..o + FRAME])
    }
}

/// The AUDIO tracks of the track records, in track order, with the CHD frame of each one's INDEX 01; a track not
/// within the `stored` frames of the CHD (a corrupt record's FRAMES or PREGAP) is left out, and the ones after it.
fn audio_tracks(records: &[String], stored: u32) -> Vec<(AudioTrack, u32)> {
    let mut tracks: Vec<(u32, String, u32, u32, bool)> = records
        .iter()
        .filter_map(|r| {
            let f = |k: &str| r.split_whitespace().find_map(|w| w.strip_prefix(k)).unwrap_or("").to_string();
            let n: u32 = f("TRACK:").parse().ok()?;
            let frames: u32 = f("FRAMES:").parse().ok()?;
            let pregap: u32 = f("PREGAP:").parse().unwrap_or(0);
            Some((n, f("TYPE:"), frames, pregap, f("PGTYPE:").starts_with('V')))
        })
        .collect();
    tracks.sort_by_key(|t| t.0);
    let (mut at, mut out) = (0u32, Vec::new());
    for (n, ty, frames, pregap, pg_data) in tracks {
        let skip = if pg_data { pregap.min(frames) } else { 0 };
        let Some(end) = at.checked_add(frames).filter(|&e| e <= stored) else { break };
        if ty == "AUDIO" {
            out.push((AudioTrack { number: n, sectors: frames - skip }, at + skip));
        }
        // padded to 4 frames (at most 3 past `end`, within u32 as `stored` is)
        at = end.div_ceil(4) * 4;
    }
    out
}

impl<R: Read + Seek + crate::disc::MaybeSend> SectorReader for ChdImage<R> {
    fn raw(&self) -> bool {
        self.raw
    }
    fn sectors(&self) -> u32 {
        self.frames
    }
    fn source(&self) -> &'static str {
        "chd"
    }
    fn read_sector(&mut self, lba: u32, out: &mut [u8]) -> Result<(), DiscError> {
        if lba >= self.frames {
            return Err(DiscError::Format(format!("sector {lba} past the end of the image ({} sectors)", self.frames)));
        }
        let n = if self.raw { RAW_SECTOR } else { ISO_SECTOR };
        let f = self.frame(lba)?;
        out[..n].copy_from_slice(&f[..n]);
        Ok(())
    }
    fn audio_tracks(&self) -> Vec<AudioTrack> {
        self.audio.iter().map(|a| a.0).collect()
    }
    fn read_audio(&mut self, track: u32, sector: u32, out: &mut [u8]) -> Result<(), DiscError> {
        let (t, at) = *self.audio.iter().find(|a| a.0.number == track).ok_or_else(|| DiscError::Format(format!("no CD-DA track {track}")))?;
        if sector >= t.sectors {
            return Err(DiscError::Format(format!("sector {sector} past the end of track {track}")));
        }
        let f = self.frame(at + sector)?;
        // big-endian samples in a CHD
        for (o, s) in out[..RAW_SECTOR].as_chunks_mut::<2>().0.iter_mut().zip(f[..RAW_SECTOR].as_chunks::<2>().0) {
            *o = [s[1], s[0]];
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::disc::{AudioTrack, Disc, RAW_SECTOR, VOLUME_ID, open_image, tests::tiny_iso};

    /// An uncompressed CHD v5 (codec none) of a raw image: 124-byte header, map of hunk numbers, hunks of 8 frames
    /// (2352 + 96 subcode bytes), one CHT2 track record.
    fn uncompressed_chd(raw: &[u8]) -> Vec<u8> {
        let frames = raw.len() / RAW_SECTOR;
        chd_of(raw, &[format!("TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:{frames} PREGAP:0 PGTYPE:MODE1 PGSUB:RW POSTGAP:0")])
    }

    /// An uncompressed CHD v5 of the frames `raw` with these CHT2 records (chained).
    fn chd_of(raw: &[u8], records: &[String]) -> Vec<u8> {
        const FRAME: usize = 2448;
        let frames = raw.len() / RAW_SECTOR;
        let hunk = 8 * FRAME;
        let hunks = frames.div_ceil(8);
        let mut data = vec![0u8; (1 + hunks) * hunk];
        for f in 0..frames {
            let o = hunk + f * FRAME;
            data[o..o + RAW_SECTOR].copy_from_slice(&raw[f * RAW_SECTOR..(f + 1) * RAW_SECTOR]);
        }
        data[..8].copy_from_slice(b"MComprHD");
        data[8..12].copy_from_slice(&124u32.to_be_bytes());
        data[12..16].copy_from_slice(&5u32.to_be_bytes());
        data[32..40].copy_from_slice(&((hunks * hunk) as u64).to_be_bytes());
        data[40..48].copy_from_slice(&124u64.to_be_bytes());
        let meta = data.len();
        data[48..56].copy_from_slice(&(meta as u64).to_be_bytes());
        data[56..60].copy_from_slice(&(hunk as u32).to_be_bytes());
        data[60..64].copy_from_slice(&(FRAME as u32).to_be_bytes());
        for h in 0..hunks {
            data[124 + 4 * h..128 + 4 * h].copy_from_slice(&(1 + h as u32).to_be_bytes());
        }
        for (k, r) in records.iter().enumerate() {
            let text = format!("{r}\0");
            let next = if k + 1 < records.len() { (data.len() + 16 + text.len()) as u64 } else { 0 };
            data.extend_from_slice(b"CHT2");
            data.extend_from_slice(&(text.len() as u32).to_be_bytes());
            data.extend_from_slice(&next.to_be_bytes());
            data.extend_from_slice(text.as_bytes());
        }
        data
    }

    #[test]
    fn chd_audio_tracks() {
        // track 1: the tiny image (24 frames); track 2 audio with a 2-frame pregap stored (PGTYPE:V), 5 frames in all;
        // track 3 audio, 3 frames, no pregap data; each track padded to 4 frames
        let data = tiny_iso(true, VOLUME_ID);
        let t1 = data.len() / RAW_SECTOR;
        let audio = |first: u8, n: usize| -> Vec<u8> { (0..n).flat_map(|k| (0..RAW_SECTOR / 2).flat_map(move |i| [first + k as u8, i as u8])).collect() };
        let mut raw = data.clone();
        raw.resize(t1.div_ceil(4) * 4 * RAW_SECTOR, 0);
        raw.extend(audio(10, 5));
        raw.resize(raw.len() + 3 * RAW_SECTOR, 0);
        raw.extend(audio(20, 3));
        let recs = [
            format!("TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:{t1} PREGAP:0 PGTYPE:MODE1 PGSUB:RW POSTGAP:0"),
            "TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:5 PREGAP:2 PGTYPE:VAUDIO PGSUB:RW POSTGAP:0".to_string(),
            "TRACK:3 TYPE:AUDIO SUBTYPE:NONE FRAMES:3 PREGAP:150 PGTYPE:AUDIO PGSUB:RW POSTGAP:0".to_string(),
        ];
        let mut img = open_image(std::io::Cursor::new(chd_of(&raw, &recs))).unwrap();
        assert_eq!(img.sectors() as usize, t1);
        assert_eq!(img.audio_tracks(), [AudioTrack { number: 2, sectors: 3 }, AudioTrack { number: 3, sectors: 3 }]);
        let mut s = vec![0u8; RAW_SECTOR];
        // track 2's INDEX 01 is its third frame (10 + 2); the samples come back little-endian
        img.read_audio(2, 0, &mut s).unwrap();
        assert_eq!((s[0], s[1], s[2], s[3]), (0, 12, 1, 12));
        img.read_audio(3, 2, &mut s).unwrap();
        assert_eq!(s[1], 22);
        assert!(img.read_audio(3, 3, &mut s).is_err());
        assert_eq!(Disc::open(img).unwrap().read("/CMN/A.DAT").unwrap(), b"abc");
    }

    #[test]
    fn chd_audio_records_past_the_stored_frames_are_left_out() {
        // a corrupt or crafted record (FRAMES:4000000000) is not a 9 TB track, and nothing after it is read
        let data = tiny_iso(true, VOLUME_ID);
        let t1 = data.len() / RAW_SECTOR;
        let recs = [
            format!("TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:{t1} PREGAP:0 PGTYPE:MODE1 PGSUB:RW POSTGAP:0"),
            "TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:4000000000 PREGAP:0 PGTYPE:AUDIO PGSUB:RW POSTGAP:0".to_string(),
            "TRACK:3 TYPE:AUDIO SUBTYPE:NONE FRAMES:4294967295 PREGAP:0 PGTYPE:AUDIO PGSUB:RW POSTGAP:0".to_string(),
        ];
        let img = open_image(std::io::Cursor::new(chd_of(&data, &recs))).unwrap();
        assert_eq!(img.audio_tracks(), []);
        assert_eq!(super::audio_tracks(&recs[1..], u32::MAX - 3), [(AudioTrack { number: 2, sectors: 4_000_000_000 }, 0)]);
    }

    #[test]
    fn chd_frames() {
        let raw = tiny_iso(true, VOLUME_ID);
        let img = open_image(std::io::Cursor::new(uncompressed_chd(&raw))).unwrap();
        assert!(img.raw());
        assert_eq!(img.sectors() as usize, raw.len() / RAW_SECTOR);
        let mut d = Disc::open(img).unwrap();
        assert_eq!(d.read("/CMN/A.DAT").unwrap(), b"abc");
        assert_eq!(d.raw_sector(23).unwrap(), &raw[23 * RAW_SECTOR..24 * RAW_SECTOR]);
    }
}
