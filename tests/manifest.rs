//! The manifest.json the converter writes, against the rules a reader of the data relies on (`format` in
//! FORMAT_MIN..=FORMAT_MAX, `source`, `xa`, `content_id`, `files: [{path, size, crc32}]` with safe paths, the required
//! runtime files, the index keys; no engine/exe.bin or tables.bin), and
//! engine/tables.okt against the disc's executable (its text table against `fixtures/text_strings.txt`). Runs on
//! BR2_DISC (and BR2_ISO, an ISO image, if set).

use ok_br2_converter::json::Json;
use ok_br2_converter::okt;
use ok_br2_converter::text::LOAD;
use ok_br2_converter::{Converter, Disc, MemorySink, Options, crc32, cue_data_track, open_image};
use std::path::PathBuf;

const FORMAT_MIN: i64 = 1;
const FORMAT_MAX: i64 = 3;
const CHARACTERS: [&str; 11] = ["YUG", "ALI", "LON", "ULI", "MAR", "BUS", "BAK", "JEN", "STU", "GAD", "RON"];
const STAGES: [&str; 13] = ["ST0", "ST1", "ST2", "ST3", "ST4", "ST5", "ST6", "ST7", "ST8", "ST9", "STA", "STB", "STC"];
const MOVIES: [&str; 4] = ["USAOP", "BR", "ED1", "ED2"];

fn safe_path(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains(':')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !reserved(c))
}

/// A name Windows keeps for a device (CON, PRN, AUX, NUL, COM1..9, LPT1..9, with or without an extension): a folder
/// or file of that name cannot be created there (Windows 10: "The directory name is invalid", os error 267).
fn reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit() && stem.as_bytes()[3] != b'0')
}

#[test]
fn reserved_names() {
    for n in ["con", "CON", "con.dat", "Aux", "nul.txt", "prn", "COM1", "lpt9.bin"] {
        assert!(reserved(n), "{n}");
    }
    for n in ["continue", "cont", "icon", "com", "com0", "com10", "lpt", "YUGCON.DAT", "console"] {
        assert!(!reserved(n), "{n}");
    }
    assert!(!safe_path("engine/con/YUGCON.DAT") && safe_path("engine/continue/YUGCON.DAT"));
}

fn required() -> Vec<String> {
    let mut v: Vec<String> =
        ["manifest.json", "engine/INIT.DAT", "engine/tables.okt", "engine/index.json", "engine/CMN.ACT", "engine/eft/EFT.DAT"].map(String::from).into();
    v.extend(["INIT", "USATITLE", "SE00", "P00"].map(|n| format!("engine/cmn/{n}.DAT")));
    // the continue screens' pictures (not under "con": a reserved name on Windows)
    v.extend(CHARACTERS.map(|c| format!("engine/continue/{c}CON.DAT")));
    for c in CHARACTERS {
        v.push(format!("engine/{c}.ACT"));
        v.push(format!("engine/eft/{c}EFT.DAT"));
        for k in 1..=4 {
            v.push(format!("engine/{c}{k}P.ANI"));
            v.push(format!("characters/{c}_{k}P_human.glb"));
            v.push(format!("characters/{c}_{k}P_beast.glb"));
        }
    }
    for s in STAGES {
        v.push(format!("stages/{s}.glb"));
        v.push(format!("engine/stage/{s}.DAT"));
    }
    v.extend(MOVIES.map(|m| format!("video/{m}.bsf")));
    v
}

fn reader(path: &str) -> Box<dyn ok_br2_converter::SectorReader> {
    let path = PathBuf::from(path);
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cue")) {
        let (bin, offset) = cue_data_track(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let f = std::fs::File::open(path.parent().unwrap().join(bin)).unwrap();
        Box::new(ok_br2_converter::FileImage::raw(std::io::BufReader::new(f), offset).unwrap()) as Box<dyn ok_br2_converter::SectorReader>
    } else {
        open_image(std::io::BufReader::new(std::fs::File::open(&path).unwrap())).unwrap()
    }
}

fn convert(path: &str) -> MemorySink {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8);
    convert_with(path, Options { viewer: false, threads })
}

fn convert_with(path: &str, opts: Options) -> MemorySink {
    let mut sink = MemorySink::default();
    Converter::new(Disc::open(reader(path)).unwrap(), opts).run(&mut sink, &mut |_| true).unwrap();
    sink
}

/// The manifest's `content_id`: a number of 1..=u32::MAX.
fn content_id(m: &Json) -> i64 {
    let id = m.get("content_id").and_then(Json::as_i64).expect("content_id is a number");
    assert!((1..=u32::MAX as i64).contains(&id), "content_id {id}");
    id
}

fn check(sink: &MemorySink, iso: bool, exe: &[u8]) {
    let m = Json::parse(std::str::from_utf8(&sink.files["manifest.json"]).unwrap()).unwrap();
    let f = m.get("format").and_then(Json::as_i64).expect("format is a number");
    assert!((FORMAT_MIN..=FORMAT_MAX).contains(&f), "format {f}");
    let source = m.get("source").and_then(Json::as_str).unwrap();
    assert!(["bin", "chd", "iso"].contains(&source));
    assert_eq!(source == "iso", iso);
    assert_eq!(m.get("xa"), Some(&Json::Bool(!iso)));
    // the content id: the converter's formula over the files written
    let written = sink.files.iter().filter(|(p, _)| *p != "manifest.json").map(|(p, d)| (p.clone(), (d.len() as u64, crc32(d)))).collect();
    assert_eq!(content_id(&m), ok_br2_converter::content_id(&written) as i64);
    // every written file listed once with its size and CRC-32, manifest.json not
    let files = m.get("files").and_then(Json::as_arr).unwrap();
    let mut listed = std::collections::BTreeSet::new();
    for e in files {
        let p = e.get("path").and_then(Json::as_str).unwrap();
        assert!(safe_path(p), "{p}");
        let data = sink.files.get(p).unwrap_or_else(|| panic!("{p} listed but not written"));
        assert_eq!(e.get("size").and_then(Json::as_i64), Some(data.len() as i64), "{p}");
        let c = e.get("crc32").and_then(Json::as_i64).unwrap();
        assert!((0..=u32::MAX as i64).contains(&c));
        assert_eq!(c as u32, crc32(data), "{p}");
        assert!(listed.insert(p.to_string()), "{p} listed twice");
    }
    for p in sink.files.keys().filter(|p| *p != "manifest.json") {
        assert!(listed.contains(p), "{p} written but not listed");
    }
    assert!(!listed.contains("manifest.json"));
    for p in required() {
        assert!(sink.files.contains_key(&p), "required {p} missing");
    }
    // the index keys
    assert_eq!(m.get("engine").and_then(Json::as_str), Some("engine/index.json"));
    let stages = m.get("stages").and_then(Json::as_arr).unwrap();
    for s in STAGES {
        let e = stages.iter().find(|e| e.get("code").and_then(Json::as_str) == Some(s)).unwrap();
        assert!(sink.files.contains_key(e.get("file").and_then(Json::as_str).unwrap()));
    }
    assert_eq!(m.get("characters").and_then(Json::as_arr).map(Vec::len), Some(11));
    let xa = sink.files.keys().filter(|p| p.ends_with(".xa")).count();
    assert_eq!(xa, if iso { 0 } else { 43 + 4 });
    // every table of tables.okt but the text is the executable's bytes at its origin; the executable itself is not
    // written
    assert!(!sink.files.contains_key("engine/exe.bin") && !sink.files.contains_key("engine/tables.bin"));
    let tables = okt::read(&sink.files["engine/tables.okt"]).unwrap();
    assert_eq!(tables.len(), ok_br2_converter::gamedata::TABLES.len() + 1);
    let (text, tables) = tables.split_last().unwrap();
    for t in tables {
        let o = t.origin.checked_sub(LOAD).unwrap_or_else(|| panic!("{}: origin below the EXE", t.name)) as usize;
        assert_eq!(Some(&t.bytes[..]), exe.get(o..o + t.bytes.len()), "{}", t.name);
    }
    // the tables at hand-found addresses: their bytes as the disc has them
    let mode_half = tables.iter().find(|t| t.name == "menu.mode_half").expect("menu.mode_half");
    assert_eq!(mode_half.bytes, [48, 16, 40, 60, 40, 48, 52]);
    let frame = tables.iter().find(|t| t.name == "hud.frame").expect("hud.frame");
    assert_eq!(frame.bytes[..8], [0, 160, 8, 8, 8, 160, 8, 8]);
    assert_eq!(frame.bytes[24..], [48, 160, 8, 7, 56, 160, 8, 7]);
    // the text: every entry's key and length as the fixture lists them, in its order, and the CRC-32 of each group of
    // entries (a list, the single texts: each text and its NUL) at the group's `crc` line
    assert_eq!(text.name, "text.strings");
    let mut entries = text.bytes.split(|&b| b == 0);
    let mut group = Vec::new();
    let golden = include_str!("fixtures/text_strings.txt");
    for l in golden.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (key, v) = l.split_once(' ').unwrap_or_else(|| panic!("fixture line {l:?}"));
        if key == "crc" {
            assert_eq!(format!("{:08x}", crc32(&group)), v, "the group ending before fixture line {l:?}");
            group.clear();
            continue;
        }
        assert_eq!(entries.next(), Some(key.as_bytes()));
        let t = entries.next().unwrap_or_else(|| panic!("{key}: no text"));
        assert_eq!(t.len().to_string(), v, "{key}");
        group.extend(t);
        group.push(0);
    }
    assert!(group.is_empty(), "entries after the fixture's last crc line");
    assert_eq!(entries.next(), Some(&[][..]));
    assert_eq!(entries.next(), None);
}

#[test]
fn manifest_meets_the_rules() {
    let Ok(disc) = std::env::var("BR2_DISC") else {
        eprintln!("BR2_DISC not set: skipped");
        return;
    };
    let exe = Disc::open(reader(&disc)).unwrap().read("/SCUS_944.24").unwrap();
    check(&convert(&disc), false, &exe);
    if let Ok(iso) = std::env::var("BR2_ISO") {
        let s = convert(&iso);
        check(&s, true, &exe);
        // the movies' frames from an ISO are the same as from the raw image
        let raw = convert(&disc);
        for m in MOVIES {
            let p = format!("video/{m}.bsf");
            assert_eq!(s.files[&p], raw.files[&p], "{p}");
        }
    }
}

/// Two conversions of the disc give the same content id and the same covered files, whatever the threads and the
/// viewer option (one on 8 threads without the viewer, one on 1 thread with it); an ISO image (BR2_ISO) gives the
/// disc's id too (`docs/content_id.md`).
#[test]
fn content_id_is_stable() {
    let Ok(disc) = std::env::var("BR2_DISC") else {
        eprintln!("BR2_DISC not set: skipped");
        return;
    };
    let (a, b) = (convert(&disc), convert_with(&disc, Options { viewer: true, threads: 1 }));
    let id = |s: &MemorySink| content_id(&Json::parse(std::str::from_utf8(&s.files["manifest.json"]).unwrap()).unwrap());
    assert_eq!(id(&a), id(&b));
    for (p, d) in a.files.iter().filter(|(p, _)| ok_br2_converter::in_content(p)) {
        assert!(b.files.get(p) == Some(d), "{p} differs between two conversions");
    }
    if let Ok(iso) = std::env::var("BR2_ISO") {
        assert_eq!(id(&convert(&iso)), id(&a), "the ISO image's content id");
    }
}
