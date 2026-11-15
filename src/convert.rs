//! The conversion: a list of steps (one character costume, one arena, one music file, …) run one at a time
//! (`Converter::step`, for a caller that draws progress between steps, e.g. the browser) or all at once
//! (`Converter::run`, on threads with the `threads` feature).

use crate::audio::{self, XaWriter};
use crate::disc::{Disc, DiscError};
use crate::gamedata;
use crate::images;
use crate::json::{Json, Obj};
use crate::lz::read_container;
use crate::model::{self, Colors};
use crate::stages;
use crate::text;
use crate::vram::Vram;
use crate::zlib::{crc32, crc32_update};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The data format the converter writes (`manifest.json` `format`), for a reader to check. 2: the static tables
/// `engine/tables.okt` (`docs/tables.md`); no `engine/exe.bin` / `engine/tables.bin` any more. 3: `tables.okt` also holds the game's text (`text.strings`) and the front end's, the HUD's and the camera's
/// tables.
pub const FORMAT_VERSION: u32 = 3;

/// A written file the content id covers (`content_id`): the files a game's simulation reads, under `engine/`
/// (the disc's files and `tables.okt`), `characters/` and `stages/` (the fighters' skeletons and the stages' models and
/// debris), and the movies' frames `video/*.bsf` (their lengths time the front end); not the music, the movies' sound,
/// the viewer's files.
pub fn in_content(path: &str) -> bool {
    path.starts_with("engine/") || path.starts_with("characters/") || path.starts_with("stages/") || (path.starts_with("video/") && path.ends_with(".bsf"))
}

/// The data's content id (`manifest.json` `content_id`, `docs/content_id.md`): CRC-32 of the lines "path:crc32\n"
/// (the CRC as 8 lower-case hex digits) of the written files `in_content` covers, sorted by path, at least 1: two
/// conversions with the same id hold the same simulation data.
pub fn content_id(written: &BTreeMap<String, (u64, u32)>) -> u32 {
    use std::fmt::Write;
    let mut text = String::new();
    for (p, (_, crc)) in written.iter().filter(|(p, _)| in_content(p)) {
        let _ = writeln!(text, "{p}:{crc:08x}");
    }
    crc32(text.as_bytes()).max(1)
}

/// Where the converted files go. `put` replaces an existing file of the same path.
pub trait Sink {
    fn put(&mut self, path: &str, data: &[u8]) -> Result<(), String>;
}

/// A sink that keeps everything in memory (tests, the web build).
#[derive(Default)]
pub struct MemorySink {
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Sink for MemorySink {
    fn put(&mut self, path: &str, data: &[u8]) -> Result<(), String> {
        self.files.insert(path.to_string(), data.to_vec());
        Ok(())
    }
}

/// Optional extras that need outside tools (the CLI's ffmpeg): previews for the viewer.
pub trait Extras: crate::disc::MaybeSend {
    /// A music track as a 16-bit stereo 37.8 kHz WAV → a preview file (path under the output, bytes).
    fn music_preview(&mut self, base: &str, wav: &[u8]) -> Option<(String, Vec<u8>)>;
    /// An FMV's raw 2352-byte sectors → files (MP4, poster) and its manifest `videos` entry.
    fn video(&mut self, base: &str, source: &str, raw: &[u8]) -> Option<(Vec<(String, Vec<u8>)>, Json)>;
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Also the viewer's files: pictures, game-logic listings, text, sound-effect WAVs, index.html.
    pub viewer: bool,
    /// Worker threads for `run` (1 = all on the calling thread). Ignored without the `threads` feature.
    pub threads: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { viewer: false, threads: 1 }
    }
}

#[derive(Debug)]
pub enum Error {
    Disc(DiscError),
    /// Data on the disc the converter cannot handle (the step and what).
    Data(String),
    Sink(String),
    Cancelled,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Disc(e) => write!(f, "{e}"),
            Error::Data(e) => write!(f, "conversion failed: {e}"),
            Error::Sink(e) => write!(f, "could not write: {e}"),
            Error::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for Error {}

impl From<DiscError> for Error {
    fn from(e: DiscError) -> Error {
        Error::Disc(e)
    }
}

/// Progress: the step being done and how many are done of how many.
#[derive(Clone, Debug)]
pub struct Progress {
    pub step: String,
    pub done: usize,
    pub total: usize,
}

/// What a finished conversion wrote.
#[derive(Clone, Debug)]
pub struct Summary {
    /// path → (size, CRC-32), manifest.json not included
    pub files: BTreeMap<String, (u64, u32)>,
    /// False for an ISO image: no music, no movie sound (the movies' frames are there).
    pub xa: bool,
    /// "bin", "chd" or "iso"
    pub source: &'static str,
}

pub const CHARACTERS: [(&str, &str); 11] = [
    ("YUG", "Yugo"),
    ("ALI", "Alice"),
    ("LON", "Long"),
    ("ULI", "Uriko"),
    ("MAR", "Shina"),
    ("BUS", "Busuzima"),
    ("BAK", "Bakuryu"),
    ("JEN", "Jenny"),
    ("STU", "Stun"),
    ("GAD", "Gado"),
    ("RON", "Shenlong"),
];

const STAGE_NAMES: [&str; 13] = [
    "City",
    "Aquarium",
    "Tatami",
    "Beach",
    "Room",
    "Dome",
    "Lab",
    "Building 1",
    "Building 2",
    "Extra stage 1",
    "Extra stage 2",
    "Extra stage 3",
    "Extra stage 4",
];

pub const MOVIES: [&str; 4] = ["USAOP", "BR", "ED1", "ED2"];

// VRAM placement by container slot (the coordinates stored in the files are stale)
const CHAR_SLOTS: [(usize, (usize, usize)); 5] = [(4, (512, 0)), (5, (576, 0)), (6, (640, 0)), (7, (0, 480)), (8, (16, 480))];

fn stage_slots() -> Vec<(usize, (usize, usize))> {
    let mut v = vec![(18, (768, 256)), (19, (704, 256))];
    for k in 0..8 {
        v.push((32 + k, (256 + 16 * k, 480)));
    }
    v
}

fn load_vram(vram: &mut Vram, ents: &[Option<Vec<u8>>], slots: &[(usize, (usize, usize))]) {
    for &(i, at) in slots {
        if let Some(Some(e)) = ents.get(i) {
            vram.load_tim(e, Some(at));
        }
    }
}

fn is_hmd(e: &Option<Vec<u8>>) -> bool {
    e.as_ref().is_some_and(|e| e.len() > 16 && u32::from_le_bytes(e[..4].try_into().unwrap()) == 0x50)
}

#[derive(Clone, Debug)]
enum Task {
    ReadFiles,
    Winding,
    /// fighter, costume (1..4), form (0 human, 1 beast)
    Character(usize, u32, usize),
    Stage(usize),
    Raizing,
    Pictures,
    TwoD,
    Text,
    GameData,
    Sfx,
    Runtime,
    StoryPictures,
    Music(String),
    Video(&'static str),
    Finish,
}

impl Task {
    fn name(&self) -> String {
        match self {
            Task::ReadFiles => "reading the disc".into(),
            Task::Winding => "checking the models".into(),
            Task::Character(c, k, f) => format!("fighter {} {}P {}", CHARACTERS[*c].1, k, ["human", "beast"][*f]),
            Task::Stage(s) => format!("arena {}", STAGE_NAMES[*s]),
            Task::Raizing => "boot logo".into(),
            Task::Pictures => "pictures".into(),
            Task::TwoD => "sprites and text pages".into(),
            Task::Text => "game text".into(),
            Task::GameData => "game-logic listings".into(),
            Task::Sfx => "sound effects".into(),
            Task::Runtime => "runtime tables".into(),
            Task::StoryPictures => "story pictures".into(),
            Task::Music(n) => format!("music {}", basename(n)),
            Task::Video(n) => format!("movie {n}"),
            Task::Finish => "manifest".into(),
        }
    }
    /// Needs the disc reader (runs on the calling thread).
    fn disc(&self) -> bool {
        matches!(self, Task::ReadFiles | Task::Music(_) | Task::Video(_) | Task::Finish)
    }
}

/// A step's result: files and its piece of the manifest.
#[derive(Default)]
struct Out {
    files: Vec<(String, Vec<u8>)>,
    piece: Option<Json>,
}

/// The conversion of one disc.
pub struct Converter {
    disc: Disc,
    opts: Options,
    ctx: Arc<Ctx>,
    tasks: Vec<Task>,
    next: usize,
    pieces: Vec<Option<Json>>,
    written: BTreeMap<String, (u64, u32)>,
    extras: Option<Box<dyn Extras>>,
}

impl Drop for Converter {
    fn drop(&mut self) {
        crate::png::cache_end();
    }
}

impl Converter {
    pub fn new(disc: Disc, opts: Options) -> Converter {
        let raw = disc.raw();
        let mut tasks = vec![Task::ReadFiles, Task::Winding];
        for c in 0..CHARACTERS.len() {
            for k in 1..=4 {
                tasks.push(Task::Character(c, k, 0));
                tasks.push(Task::Character(c, k, 1));
            }
        }
        for s in 0..13 {
            tasks.push(Task::Stage(s));
        }
        tasks.push(Task::Raizing);
        if opts.viewer {
            tasks.extend([Task::Pictures, Task::TwoD, Task::Text, Task::GameData, Task::Sfx]);
        }
        tasks.extend([Task::Runtime, Task::StoryPictures]);
        if raw {
            for (name, _) in disc.files.iter().filter(|(n, _)| n.ends_with(".XA")) {
                tasks.push(Task::Music(name.clone()));
            }
        }
        // the movies' frames are Form 1 sectors: an ISO has them too (without the sound)
        for m in MOVIES {
            tasks.push(Task::Video(m));
        }
        tasks.push(Task::Finish);
        let n = tasks.len();
        crate::png::cache_begin();
        Converter { disc, opts, ctx: Arc::new(Ctx::default()), tasks, next: 0, pieces: vec![None; n], written: BTreeMap::new(), extras: None }
    }

    /// Previews made by outside tools (the CLI's ffmpeg); only with `viewer`.
    pub fn set_extras(&mut self, e: Box<dyn Extras>) {
        self.extras = Some(e);
    }

    pub fn total(&self) -> usize {
        self.tasks.len()
    }
    pub fn done(&self) -> usize {
        self.next
    }
    pub fn progress(&self) -> Progress {
        Progress { step: self.tasks.get(self.next).map(Task::name).unwrap_or_else(|| "done".into()), done: self.next, total: self.tasks.len() }
    }

    fn put(&mut self, sink: &mut dyn Sink, path: &str, data: &[u8]) -> Result<(), Error> {
        sink.put(path, data).map_err(Error::Sink)?;
        self.written.insert(path.to_string(), (data.len() as u64, crc32(data)));
        Ok(())
    }

    fn finish_out(&mut self, sink: &mut dyn Sink, i: usize, out: Out) -> Result<(), Error> {
        for (p, d) in &out.files {
            self.put(sink, p, d)?;
        }
        self.pieces[i] = out.piece;
        Ok(())
    }

    /// Runs the next step; true when the conversion is complete.
    pub fn step(&mut self, sink: &mut dyn Sink) -> Result<bool, Error> {
        let Some(task) = self.tasks.get(self.next).cloned() else { return Ok(true) };
        let i = self.next;
        let out =
            if task.disc() { self.disc_task(&task, sink)? } else { pure_task(&task, &self.ctx).map_err(|e| Error::Data(format!("{}: {e}", task.name())))? };
        self.finish_out(sink, i, out)?;
        self.next += 1;
        Ok(self.next == self.tasks.len())
    }

    /// Runs every step (on `opts.threads` threads with the `threads` feature); `progress` returns false to cancel.
    pub fn run(mut self, sink: &mut dyn Sink, progress: &mut dyn FnMut(&Progress) -> bool) -> Result<Summary, Error> {
        // the first steps (disc read, winding) on this thread: everything after needs them
        while self.next < 2 {
            if !progress(&self.progress()) {
                return Err(Error::Cancelled);
            }
            self.step(sink)?;
        }
        #[cfg(all(feature = "threads", not(target_arch = "wasm32")))]
        if self.opts.threads > 1 {
            return self.run_threads(sink, progress);
        }
        loop {
            if !progress(&self.progress()) {
                return Err(Error::Cancelled);
            }
            if self.step(sink)? {
                break;
            }
        }
        Ok(self.summary())
    }

    #[cfg(all(feature = "threads", not(target_arch = "wasm32")))]
    fn run_threads(mut self, sink: &mut dyn Sink, progress: &mut dyn FnMut(&Progress) -> bool) -> Result<Summary, Error> {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::mpsc;
        let start = self.next;
        let last = self.tasks.len() - 1; // Finish, after everything else
        let pure: Vec<usize> = (start..last).filter(|&i| !self.tasks[i].disc()).collect();
        let discs: Vec<usize> = (start..last).filter(|&i| self.tasks[i].disc()).collect();
        let cursor = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let (tx, rx) = mpsc::channel::<(usize, Result<Out, String>)>();
        let ctx = self.ctx.clone();
        let tasks = self.tasks.clone();
        let mut done = start;
        let total = self.tasks.len();
        let result: Result<(), Error> = std::thread::scope(|s| {
            for _ in 0..self.opts.threads {
                let tx = tx.clone();
                let (cursor, stop, pure, ctx, tasks) = (&cursor, &stop, &pure, &ctx, &tasks);
                s.spawn(move || {
                    loop {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let k = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(&i) = pure.get(k) else { break };
                        let r = pure_task(&tasks[i], ctx);
                        if tx.send((i, r)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(tx);
            let r = (|| -> Result<(), Error> {
                let mut pending = discs.iter();
                loop {
                    // a disc step on this thread, then whatever the workers finished meanwhile
                    let next_disc = pending.next();
                    if let Some(&i) = next_disc {
                        let out = self.disc_task(&tasks[i], sink)?;
                        self.finish_out(sink, i, out)?;
                        done += 1;
                        if !progress(&Progress { step: tasks[i].name(), done, total }) {
                            return Err(Error::Cancelled);
                        }
                    }
                    loop {
                        let msg = if next_disc.is_some() { rx.try_recv().ok() } else { rx.recv().ok() };
                        let Some((i, r)) = msg else { break };
                        let out = r.map_err(|e| Error::Data(format!("{}: {e}", tasks[i].name())))?;
                        self.finish_out(sink, i, out)?;
                        done += 1;
                        if !progress(&Progress { step: tasks[i].name(), done, total }) {
                            return Err(Error::Cancelled);
                        }
                    }
                    if next_disc.is_none() {
                        return Ok(());
                    }
                }
            })();
            if r.is_err() {
                stop.store(true, Ordering::Relaxed);
                while rx.recv().is_ok() {}
            }
            r
        });
        result?;
        if done != total - 1 {
            return Err(Error::Data("a step did not finish".into()));
        }
        self.next = last;
        self.step(sink)?;
        Ok(self.summary())
    }

    fn summary(&self) -> Summary {
        Summary { files: self.written.clone(), xa: self.disc.raw(), source: self.disc.source() }
    }

    fn disc_task(&mut self, task: &Task, sink: &mut dyn Sink) -> Result<Out, Error> {
        let mut out = Out::default();
        match task {
            Task::ReadFiles => {
                let mut files = BTreeMap::new();
                let names: Vec<String> = self.disc.files.keys().cloned().collect();
                for n in names {
                    if n.ends_with(".XA") || n.ends_with(".STR") || n.ends_with("/DUMMY") {
                        continue; // Form 2 audio / video streams
                    }
                    let d = self.disc.read(&n)?;
                    files.insert(n, d);
                }
                let exe = files.get("/SCUS_944.24").cloned().unwrap_or_default();
                let sound_names = text::sound_bank_names(&exe, &self.disc.order);
                let winding = stage_winding(&files).map_err(|e| Error::Data(format!("models: {e}")))?;
                self.ctx = Arc::new(Ctx { files, winding, sound_names, order: self.disc.order.clone() });
            }
            Task::Winding => {}
            Task::Music(name) => out = self.music(name)?,
            Task::Video(base) => out = self.video(base)?,
            Task::Finish => {
                self.finish(sink)?;
            }
            _ => unreachable!(),
        }
        Ok(out)
    }

    /// A stream file's sectors (lba, count of 2048-byte sectors).
    fn span(&self, name: &str) -> Result<(u32, u32), Error> {
        let e = self.disc.entry(name).ok_or_else(|| Error::Data(format!("{name} is not on the disc")))?;
        Ok((e.lba, e.size / 2048))
    }

    /// One XA file: every channel carrying audio as `audio/music/<FILE>_ch<n>.xa`.
    fn music(&mut self, name: &str) -> Result<Out, Error> {
        let (lba, n) = self.span(name)?;
        let mut chans: BTreeMap<u8, XaWriter> = BTreeMap::new();
        for s in lba..lba + n {
            let raw = self.disc.raw_sector(s)?;
            let (chan, submode, coding) = (raw[17], raw[18], raw[19]);
            if submode & 0x04 == 0 || coding != 0x01 {
                continue;
            }
            let w = chans.entry(chan).or_insert_with(|| XaWriter::new(coding));
            if !w.done {
                w.push(raw);
            }
        }
        let base = basename(name).strip_suffix(".XA").unwrap_or(basename(name));
        let mut out = Out::default();
        let mut list = Vec::new();
        for (ch, w) in chans {
            let n = w.sectors as f64 * 2016.0;
            let stemname = format!("{base}_ch{ch}");
            let rel = format!("audio/music/{stemname}.xa");
            let data = w.finish();
            let mut e = Obj::new()
                .with("file", rel.as_str())
                .with("title", stemname.replace("BR2_", "").replace('_', " "))
                .with("duration", crate::py::round(n / 37800.0, 1))
                .with("source", format!("{base}.XA"))
                .with("channel", ch);
            if self.opts.viewer
                && let Some(x) = self.extras.as_mut()
            {
                let mut pcm = Vec::new();
                audio::decode_xa(&data[audio::XA_HEADER..], &mut pcm);
                if let Some((p, b)) = x.music_preview(&stemname, &audio::wav(&pcm, 37800, 2)) {
                    e.set("preview", p.as_str());
                    out.files.push((p, b));
                }
            }
            out.files.push((rel, data));
            list.push(Json::Obj(e));
        }
        out.piece = Some(Json::Arr(list));
        Ok(out)
    }

    /// One FMV: the MDEC frames (`video/<NAME>.bsf`) and its audio (`video/<NAME>.xa`).
    fn video(&mut self, base: &str) -> Result<Out, Error> {
        let name = format!("/MOV/{base}.STR");
        let (lba, n) = self.span(&name)?;
        let mut bsf = Bsf::default();
        if !self.disc.raw() {
            // an ISO: the 2048 user bytes of each sector; the video sectors are the ones with the STR header
            for s in lba..lba + n {
                bsf.push_user((s - lba) as usize, &self.disc.sector(s)?);
            }
            return Ok(Out { files: vec![(format!("video/{base}.bsf"), bsf.finish())], piece: None });
        }
        let keep = self.opts.viewer && self.extras.is_some();
        let mut raw_all = Vec::new();
        let mut xa: Option<XaWriter> = None;
        for s in lba..lba + n {
            let raw = self.disc.raw_sector(s)?;
            if keep {
                raw_all.extend_from_slice(raw);
            }
            if raw[18] & 0x04 == 0 {
                bsf.push((s - lba) as usize, raw);
                continue;
            }
            let w = xa.get_or_insert_with(|| XaWriter::new(raw[19]));
            if !w.done {
                w.push(raw);
            }
        }
        let mut out = Out::default();
        out.files.push((format!("video/{base}.bsf"), bsf.finish()));
        if let Some(w) = xa {
            out.files.push((format!("video/{base}.xa"), w.finish()));
        }
        if self.opts.viewer
            && let Some(x) = self.extras.as_mut()
            && let Some((files, entry)) = x.video(base, &name, &raw_all)
        {
            out.files.extend(files);
            out.piece = Some(entry);
        }
        Ok(out)
    }

    fn piece(&self, pred: impl Fn(&Task) -> bool) -> Vec<&Json> {
        self.tasks.iter().zip(&self.pieces).filter(|(t, p)| pred(t) && p.is_some()).map(|(_, p)| p.as_ref().unwrap()).collect()
    }

    /// manifest.json (and the viewer page).
    fn finish(&mut self, sink: &mut dyn Sink) -> Result<(), Error> {
        if self.opts.viewer {
            self.put(sink, "index.html", crate::VIEWER_HTML.as_bytes())?;
        }
        let mut m = Obj::new().with("game", "Bloody Roar II (USA) SCUS-94424");
        // the viewer adds it to its URLs to defeat caching: a number from the files' checksums
        let mut h = 0;
        for (p, (s, c)) in &self.written {
            h = crc32_update(h, p.as_bytes());
            h = crc32_update(h, &s.to_le_bytes());
            h = crc32_update(h, &c.to_le_bytes());
        }
        m.set("version", h);
        let mut chars: Vec<Json> = Vec::new();
        for (ci, (code, name)) in CHARACTERS.iter().enumerate() {
            let mut costumes: Vec<Json> = Vec::new();
            for k in 1..=4u32 {
                if !self.ctx.files.contains_key(&format!("/{code}/{code}{k}P.DAT")) {
                    continue;
                }
                let mut models = Obj::new();
                for (f, form) in ["human", "beast"].into_iter().enumerate() {
                    if let Some(m) = self.piece(|t| matches!(t, Task::Character(c, kk, ff) if *c == ci && *kk == k && *ff == f)).first() {
                        models.set(form, (*m).clone());
                    }
                }
                costumes.push(Json::Obj(Obj::new().with("costume", k).with("models", models)));
            }
            chars.push(Json::Obj(
                Obj::new().with("code", *code).with("name", *name).with("story", images::lookup(&images::STORY_TITLES, code)).with("costumes", costumes),
            ));
        }
        m.set("characters", chars);
        let st: Vec<Json> = self.piece(|t| matches!(t, Task::Stage(_) | Task::Raizing)).into_iter().cloned().collect();
        m.set("stages", st);
        if self.opts.viewer {
            let mut imgs = Vec::new();
            for p in self.piece(|t| matches!(t, Task::Pictures | Task::TwoD)) {
                imgs.extend(p.as_arr().cloned().unwrap_or_default());
            }
            m.set("images", imgs);
        }
        let mut music = Vec::new();
        for p in self.piece(|t| matches!(t, Task::Music(_))) {
            music.extend(p.as_arr().cloned().unwrap_or_default());
        }
        let mut audio_o = Obj::new().with("music", music);
        if let Some(sfx) = self.piece(|t| matches!(t, Task::Sfx)).first() {
            audio_o.set("sfx", (*sfx).clone());
        }
        m.set("audio", audio_o);
        let videos: Vec<Json> = self.piece(|t| matches!(t, Task::Video(_))).into_iter().cloned().collect();
        if !videos.is_empty() {
            m.set("videos", videos);
        }
        if let Some(t) = self.piece(|t| matches!(t, Task::Text)).first() {
            m.set("text", (*t).clone());
        }
        if let Some(t) = self.piece(|t| matches!(t, Task::GameData)).first() {
            m.set("data", (*t).clone());
        }
        m.set("engine", "engine/index.json");
        let raw = self.disc.raw();
        // the install record: data format, source, CD-XA audio present, the content id, every file written with its size
        // and CRC-32
        m.set("format", FORMAT_VERSION);
        m.set("content_id", content_id(&self.written));
        m.set("source", self.disc.source());
        m.set("xa", raw);
        m.set(
            "converter",
            Obj::new()
                .with("name", concat!("ok_br2_converter ", env!("CARGO_PKG_VERSION")))
                .with("disc", "SCUS-94424")
                .with("music", if raw { "xa" } else { "none" })
                .with("movies", if raw { "frames and sound" } else { "frames only" })
                .with("viewer", self.opts.viewer)
                .with("checksum", "crc32"),
        );
        let files: Vec<Json> =
            self.written.iter().map(|(p, (s, c))| Json::Obj(Obj::new().with("path", p.as_str()).with("size", *s as i64).with("crc32", *c))).collect();
        m.set("files", files);
        let text = Json::Obj(m).indent(1);
        sink.put("manifest.json", text.as_bytes()).map_err(Error::Sink)
    }
}

/// The stages' common winding sign: from the fighters' lit polygons (`convert.STAGE_WINDING`).
fn stage_winding(files: &BTreeMap<String, Vec<u8>>) -> Result<i64, String> {
    let mut sum = 0;
    for (code, _) in CHARACTERS {
        for k in 1..=4 {
            let Some(d) = files.get(&format!("/{code}/{code}{k}P.DAT")) else { continue };
            let ents = read_container(d).map_err(|e| e.0.to_string())?;
            for idx in [0, 2] {
                if let Some(Some(e)) = ents.get(idx) {
                    let h = crate::hmd::Hmd::parse(e)?;
                    sum += model::winding_sign(&model::hmd_geometry(&h, None)?);
                }
            }
        }
    }
    Ok(if sum >= 0 { 1 } else { -1 })
}

/// STR video sectors (Mode 2 Form 1, 32-byte header `u16 0x0160, u16 0x8001, u16 chunk, u16 chunks, u32 frame,
/// u32 frame bytes, u16 width, u16 height`, 2016 bytes of the frame) → `.bsf`: `b"BRBS", u16 width, u16 height,
/// u32 frames`, then per frame `u32 sector, u32 length` and the bitstream.
#[derive(Default)]
pub struct Bsf {
    frames: Vec<BsfFrame>,
}

struct BsfFrame {
    no: u32,
    sector: usize,
    size: usize,
    w: u16,
    h: u16,
    chunks: BTreeMap<u16, Vec<u8>>,
}

impl Bsf {
    /// One raw 2352-byte sector, `s` = its index in the file.
    pub fn push(&mut self, s: usize, sec: &[u8]) {
        if sec[18] & 0x04 == 0 {
            self.push_user(s, &sec[24..24 + 2048]);
        }
    }

    /// A sector's 2048 user bytes (an ISO image: no subheader, so only the STR header tells a video sector).
    pub fn push_user(&mut self, s: usize, d: &[u8]) {
        let u16_at = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
        if (u16_at(0), u16_at(2)) != (0x0160, 0x8001) {
            return;
        }
        let (chunk, fno, size, w, h) = (u16_at(4), u32_at(8), u32_at(12) as usize, u16_at(16), u16_at(18));
        if self.frames.last().is_none_or(|f| f.no != fno) {
            self.frames.push(BsfFrame { no: fno, sector: s, size, w, h, chunks: BTreeMap::new() });
        }
        self.frames.last_mut().unwrap().chunks.insert(chunk, d[32..32 + 2016].to_vec());
    }

    pub fn finish(self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"BRBS");
        let (w, h) = self.frames.first().map(|f| (f.w, f.h)).unwrap_or((0, 0));
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&(self.frames.len() as u32).to_le_bytes());
        for f in &self.frames {
            let mut data: Vec<u8> = f.chunks.values().flatten().copied().collect();
            data.truncate(f.size);
            out.extend_from_slice(&(f.sector as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&data);
        }
        out
    }
}

fn ents_of(files: &BTreeMap<String, Vec<u8>>, name: &str) -> Result<Vec<Option<Vec<u8>>>, String> {
    let d = files.get(name).ok_or_else(|| format!("{name} is not on the disc"))?;
    read_container(d).map_err(|e| format!("{name}: {}", e.0))
}

fn basename(n: &str) -> &str {
    n.rsplit('/').next().unwrap_or(n)
}

/// What the steps share: the disc's files (streams excluded), read once.
#[derive(Default)]
struct Ctx {
    files: BTreeMap<String, Vec<u8>>,
    /// the stages' common winding sign
    winding: i64,
    /// sound-test names of the sound banks (disc path → name)
    sound_names: std::collections::HashMap<String, String>,
    /// the disc's files in directory order
    order: Vec<(String, crate::disc::Entry)>,
}

/// The steps that need only the files read from the disc (they may run on any thread).
fn pure_task(task: &Task, ctx: &Ctx) -> Result<Out, String> {
    let (files, winding) = (&ctx.files, ctx.winding);
    let mut out = Out::default();
    match task {
        Task::Winding => {}
        Task::Character(ci, costume, fi) => {
            let code = CHARACTERS[*ci].0;
            let fname = format!("/{code}/{code}{costume}P.DAT");
            if !files.contains_key(&fname) {
                return Ok(out);
            }
            let ents = ents_of(files, &fname)?;
            let ani = files.get(&format!("/{code}/{code}{costume}P.ANI")).ok_or("no ANI")?;
            let act = files.get(&format!("/{code}/{code}.ACT")).ok_or("no ACT")?;
            let cmn = files.get("/CMN/CMN.ACT");
            let mut vram = Vram::default();
            load_vram(&mut vram, &ents, &CHAR_SLOTS);
            let (form, idx) = [("human", 0usize), ("beast", 2usize)][*fi];
            if let Some(Some(hmd)) = ents.get(idx) {
                let name = format!("{code}_{costume}P_{form}");
                let rel = format!("characters/{name}.glb");
                let proxy = ents.get(idx + 1).and_then(|e| e.as_deref());
                let (glb, info) = model::export_character(hmd, ani, *fi, &vram, &name, Some(act), proxy, cmn.map(|v| v.as_slice()))?;
                out.files.push((rel.clone(), glb));
                let mut e = Obj::new().with("file", rel);
                e.0.extend(info.0);
                out.piece = Some(Json::Obj(e));
            }
        }
        Task::Stage(s) => {
            let st = format!("ST{s:X}");
            let fname = format!("/{st}/{st}.DAT");
            if !files.contains_key(&fname) {
                return Ok(out);
            }
            let ents = ents_of(files, &fname)?;
            let mut vram = Vram::default();
            load_vram(&mut vram, &ents, &stage_slots());
            let hmds: Vec<(usize, &[u8])> = ents.iter().enumerate().filter(|(_, e)| is_hmd(e)).map(|(i, e)| (i, e.as_deref().unwrap())).collect();
            let floor = if ents.len() > 1 { model::floor_quads(ents[0].as_deref(), ents[1].as_deref()) } else { Vec::new() };
            let (glbs, mut info) = model::export_static(&hmds, &vram, &st, &floor, winding, Colors::Clamped)?;
            for (n, d) in glbs {
                out.files.push((format!("stages/{n}"), d));
            }
            info.set("floor_tiles", floor.iter().filter(|q| model::page_loaded(&vram, q.2)).count());
            let mut e = Obj::new().with("code", st.as_str()).with("name", STAGE_NAMES[*s]).with("file", format!("stages/{st}.glb"));
            e.0.extend(info.0);
            out.piece = Some(Json::Obj(e));
            // the split stages: moved and skipped objects, semi-transparent parts
            if !stages::moved(&st).is_empty() || !stages::skipped(&st).is_empty() {
                for (n, d) in stages::export_split(&hmds, &vram, &st, &floor, false)? {
                    out.files.push((format!("stages/{n}"), d));
                }
            }
            if stages::has_semi(&hmds)? {
                for (n, d) in stages::export_split(&hmds, &vram, &st, &floor, true)? {
                    out.files.push((format!("stages/{n}"), d));
                }
                out.files.push((format!("stages/{st}_semi.glb"), stages::export_semi(&hmds, &vram, &st)?));
            }
            // the ordering-table stage files
            if ents.len() > 2 && is_hmd(&ents[2]) {
                let v1 = stages::export_ot(&ents, &vram, &st, Colors::Clamped)?;
                let v2 = stages::export_ot(&ents, &vram, &st, Colors::Unclamped)?;
                if v2 != v1 {
                    out.files.push((format!("stages/{st}_ot_v2.glb"), v2));
                }
                out.files.push((format!("stages/{st}_ot.glb"), v1));
                if ents.len() > 3 && is_hmd(&ents[3]) {
                    for (n, d) in stages::export_debris_v2(ents[3].as_deref().unwrap(), &vram, &st)? {
                        let v1name = format!("stages/{}", n.replace("_v2.glb", ".glb"));
                        let same = out.files.iter().find(|f| f.0 == v1name).is_some_and(|f| f.1 == d);
                        if !same {
                            out.files.push((format!("stages/{n}"), d));
                        }
                    }
                }
            }
        }
        Task::Raizing => {
            let Some(d) = files.get("/CMN/RAIZING.DAT") else { return Ok(out) };
            let (glbs, info) = model::export_static(&[(0, d)], &Vram::default(), "RAIZING", &[], winding, Colors::Clamped)?;
            for (n, d) in glbs {
                out.files.push((format!("stages/{n}"), d));
            }
            let mut e = Obj::new().with("code", "RAIZING").with("name", "Raizing logo (boot)").with("file", "stages/RAIZING.glb");
            e.0.extend(info.0);
            out.piece = Some(Json::Obj(e));
        }
        Task::Pictures => out.piece = Some(Json::Arr(images::pictures(files, &mut out.files))),
        Task::TwoD => out.piece = Some(Json::Arr(images::export_2d(files, &mut out.files))),
        Task::Text => {
            let exe = files.get("/SCUS_944.24").ok_or("no EXE")?;
            let items = text::extract(exe);
            out.files.push(("text/game_text.json".into(), text::items_json(&items).indent(0).into_bytes()));
            out.files.push(("text/game_text.txt".into(), text::items_txt(&items).into_bytes()));
            out.piece = Some(Json::Obj(Obj::new().with("file", "text/game_text.json").with("count", items.len())));
        }
        Task::GameData => out.piece = Some(game_data(files, &mut out.files)?),
        Task::Sfx => out.piece = Some(sfx(files, &ctx.sound_names, &mut out.files)),
        Task::Runtime => runtime(files, &mut out.files)?,
        Task::StoryPictures => {
            // the story pictures and pages, the arcade ending illustrations (strframes.export_pictures), in disc order
            for (name, _) in &ctx.order {
                let Some(data) = files.get(name) else { continue };
                let base = basename(name).split(';').next().unwrap_or("");
                let dst = if name.starts_with("/STY/PIC/") {
                    format!("engine/sty/{base}")
                } else if name.starts_with("/STY/") {
                    format!("engine/sty/{}/{base}", name.split('/').nth(2).unwrap_or(""))
                } else if base.ends_with("CON.DAT") {
                    // the disc's CON folder under another name: CON is a reserved device name on Windows, where
                    // creating a folder named "con" fails ("The directory name is invalid", os error 267)
                    format!("engine/continue/{base}")
                } else {
                    continue;
                };
                out.files.push((dst, data.clone()));
            }
        }
        _ => unreachable!("{task:?}"),
    }
    Ok(out)
}

const SND_GROUPS: [(&str, &str); 7] = [
    ("CHARA", "voices"),
    ("GOVER", "defeat voices"),
    ("STAGE", "stage"),
    ("COMMON", "common"),
    ("CHASEL", "character select"),
    ("TITLE", "title"),
    ("ENDING", "ending"),
];

/// Every VAB bank's samples as WAV (`export_audio`'s second half).
fn sfx(files: &BTreeMap<String, Vec<u8>>, official: &std::collections::HashMap<String, String>, out: &mut Vec<(String, Vec<u8>)>) -> Json {
    let mut banks = Vec::new();
    for (name, data) in files.iter().filter(|(n, _)| n.ends_with(".SND")) {
        let samples = audio::read_vab(data);
        if samples.is_empty() {
            continue;
        }
        let folder = name.trim_matches('/').split('/').next().unwrap_or("");
        let base = images::stem(name);
        let kind = SND_GROUPS.iter().find(|g| g.0 == base.split('_').next().unwrap_or("")).map(|g| g.1).unwrap_or(base);
        let rel_dir = format!("audio/sfx/{folder}_{base}");
        let mut items = Vec::new();
        for (idx, rate, pcm) in samples {
            let rel = format!("{rel_dir}/{idx:02}.wav");
            out.push((rel.clone(), audio::wav(&pcm, rate, 1)));
            items.push(Json::Obj(Obj::new().with("file", rel).with("rate", rate).with("duration", crate::py::round(pcm.len() as f64 / rate as f64, 2))));
        }
        banks.push(Json::Obj(
            Obj::new().with("bank", format!("{folder}/{base}")).with("kind", kind).with("label", official.get(name).cloned()).with("samples", items),
        ));
    }
    Json::Arr(banks)
}

/// data/ani, data/effects, data/emitters, data/effect_vm_tables.json (`gamedata.export_all`).
fn game_data(files: &BTreeMap<String, Vec<u8>>, out: &mut Vec<(String, Vec<u8>)>) -> Result<Json, String> {
    let exe = files.get("/SCUS_944.24").ok_or("no EXE")?;
    let sizes = gamedata::load_vm_tables(exe);
    let (mut ani_l, mut moves_l, mut eff_l, mut emi_l) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (name, ani) in files.iter().filter(|(n, _)| n.ends_with(".ANI")) {
        let base = images::stem(name);
        out.push((format!("data/ani/{base}.json"), gamedata::decode_ani(ani).indent(1).into_bytes()));
        ani_l.push(Json::from(format!("data/ani/{base}.json")));
        let (ent, end) = gamedata::move_entries(ani);
        let (txt, _) = gamedata::disassemble_moves(ani, &ent, end);
        out.push((format!("data/ani/{base}_moves.txt"), txt.into_bytes()));
        moves_l.push(Json::from(format!("data/ani/{base}_moves.txt")));
    }
    for (name, data) in files.iter().filter(|(n, _)| n.ends_with(".DAT")) {
        let Ok(ents) = read_container(data) else { continue };
        let st = name.trim_matches('/').replace('/', "_");
        let stem = &st[..st.len() - 4];
        for (i, e) in ents.iter().enumerate() {
            let Some(e) = e.as_ref().filter(|e| e.len() >= 28) else { continue };
            let m = u32::from_le_bytes(e[..4].try_into().unwrap());
            if m == 0x1C {
                let (js, txt) = gamedata::disassemble_effect(e, &sizes);
                out.push((format!("data/effects/{stem}_{i:02}.txt"), txt.into_bytes()));
                out.push((format!("data/effects/{stem}_{i:02}.json"), js.indent(1).into_bytes()));
                eff_l.push(Json::from(format!("data/effects/{stem}_{i:02}.txt")));
            } else if m == 0xED1C4 {
                out.push((format!("data/emitters/{stem}_{i:02}.json"), gamedata::decode_emitter(e).indent(1).into_bytes()));
                emi_l.push(Json::from(format!("data/emitters/{stem}_{i:02}.json")));
            }
        }
    }
    out.push(("data/effect_vm_tables.json".into(), gamedata::vm_tables_json(&sizes).indent(1).into_bytes()));
    Ok(Json::Obj(Obj::new().with("ani", ani_l).with("moves", moves_l).with("effects", eff_l).with("emitters", emi_l)))
}

/// The runtime data under `engine/`: the disc's raw game files and the executable's tables.
fn runtime(files: &BTreeMap<String, Vec<u8>>, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
    let mut index = Obj::new().with("fighters", Obj::new()).with("common_act", Json::Null);
    let is_stage = |n: &str| {
        let b = n.as_bytes();
        let c = |x: u8| x.is_ascii_digit() || (b'A'..=b'C').contains(&x);
        b.len() == 12 && n.starts_with("/ST") && c(b[3]) && &n[4..7] == "/ST" && c(b[7]) && n.ends_with(".DAT")
    };
    for (name, data) in files {
        let base = basename(name);
        if name.starts_with("/CMN/") && name.ends_with(".DAT") {
            // front end: title, menu emblems, select, VS backgrounds
            out.push((format!("engine/cmn/{base}"), data.clone()));
            if base != "INIT.DAT" {
                continue;
            }
        }
        if base == "INIT.DAT" {
            out.push(("engine/INIT.DAT".into(), data.clone()));
            continue;
        }
        if base.ends_with("EFT.DAT") {
            out.push((format!("engine/eft/{base}"), data.clone()));
            continue;
        }
        if is_stage(name) {
            out.push((format!("engine/stage/{base}"), data.clone()));
            continue;
        }
        if name.ends_with(".SND") {
            out.push((format!("engine/snd/{base}"), data.clone()));
            index.entry("sound_banks", Vec::<Json>::new()).arr().push(format!("engine/snd/{base}").into());
            continue;
        }
        if name.ends_with(".ANI") || name.ends_with(".ACT") {
            out.push((format!("engine/{base}"), data.clone()));
            if base == "CMN.ACT" {
                index.set("common_act", "engine/CMN.ACT");
                continue;
            }
            let code = &base[..3];
            let fi = index.get_mut("fighters").unwrap().obj().entry(code, Obj::new().with("act", Json::Null).with("ani", Obj::new())).obj();
            if name.ends_with(".ACT") {
                fi.set("act", format!("engine/{base}"));
            } else {
                fi.get_mut("ani").unwrap().obj().set(&base[3..4], format!("engine/{base}"));
            }
        }
    }
    let exe = files.get("/SCUS_944.24").ok_or("no EXE")?;
    gamedata::runtime_index(exe, &mut index);
    // the static tables and the text by name (format 3): no copy of the executable is written
    out.push(("engine/tables.okt".into(), gamedata::tables_okt(exe)?));
    out.push(("engine/index.json".into(), Json::Obj(index).indent(1).into_bytes()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Bsf;

    fn video_sector(chunk: u16, frame: u32, size: u32, fill: u8) -> Vec<u8> {
        let mut s = vec![0u8; 2352];
        s[18] = 0x48;
        s[24..26].copy_from_slice(&0x0160u16.to_le_bytes());
        s[26..28].copy_from_slice(&0x8001u16.to_le_bytes());
        s[28..30].copy_from_slice(&chunk.to_le_bytes());
        s[30..32].copy_from_slice(&2u16.to_le_bytes());
        s[32..36].copy_from_slice(&frame.to_le_bytes());
        s[36..40].copy_from_slice(&size.to_le_bytes());
        s[40..42].copy_from_slice(&320u16.to_le_bytes());
        s[42..44].copy_from_slice(&240u16.to_le_bytes());
        s[56..56 + 2016].fill(fill);
        s
    }

    /// The content id: CRC-32 of the covered files' "path:crc32" lines in path order; other files do not change it.
    #[test]
    fn content_id_covers_the_simulation_files() {
        use super::{content_id, in_content};
        use crate::zlib::crc32;
        let mut w = std::collections::BTreeMap::new();
        w.insert("engine/YUG.ACT".to_string(), (10, 0xABu32));
        w.insert("characters/YUG_1P_human.glb".to_string(), (20, 0x1234_5678));
        let id = content_id(&w);
        assert_eq!(id, crc32(b"characters/YUG_1P_human.glb:12345678\nengine/YUG.ACT:000000ab\n"));
        for p in ["audio/music/BR2_MC01_ch0.xa", "video/BR.xa", "index.html", "images/x.png"] {
            assert!(!in_content(p), "{p}");
            w.insert(p.to_string(), (1, 1));
        }
        assert_eq!(content_id(&w), id);
        for p in ["video/BR.bsf", "stages/ST0.glb", "engine/tables.okt"] {
            assert!(in_content(p), "{p}");
        }
        w.insert("video/BR.bsf".to_string(), (1, 1));
        assert_ne!(content_id(&w), id);
        assert_eq!(content_id(&Default::default()), crc32(b"").max(1));
    }

    /// Desktop / Android callers may run a conversion on a background thread.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn converter_is_send() {
        fn send<T: Send>() {}
        send::<super::Converter>();
    }

    #[test]
    fn str_frames() {
        let mut b = Bsf::default();
        // chunks out of order, an audio sector between them
        b.push(0, &video_sector(1, 7, 2100, 0xBB));
        let mut audio = vec![0u8; 2352];
        audio[18] = 0x64;
        b.push(1, &audio);
        b.push(2, &video_sector(0, 7, 2100, 0xAA));
        b.push(3, &video_sector(0, 8, 10, 0xCC));
        let f = b.finish();
        assert_eq!(&f[..4], b"BRBS");
        assert_eq!(u16::from_le_bytes([f[4], f[5]]), 320);
        assert_eq!(u32::from_le_bytes(f[8..12].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(f[12..16].try_into().unwrap()), 0); // first frame starts at sector 0
        assert_eq!(u32::from_le_bytes(f[16..20].try_into().unwrap()), 2100);
        assert_eq!((f[20], f[20 + 2015], f[20 + 2016]), (0xAA, 0xAA, 0xBB));
        let g = 20 + 2100;
        assert_eq!(u32::from_le_bytes(f[g..g + 4].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(f[g + 4..g + 8].try_into().unwrap()), 10);
        assert_eq!(f.len(), g + 8 + 10);
    }
}
