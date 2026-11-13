//! okconv-br2: the converter's command line.
//!
//! ```text
//! okconv-br2 convert IMAGE OUT [--viewer] [--no-video] [--threads N]   IMAGE: .cue, .bin, .chd or .iso
//! okconv-br2 serve DIR [--port 8000]                                   the viewer, with HTTP Range (seeking)
//! okconv-br2 check DIR                                                 every file against manifest.json's checksums
//! ```

mod ffmpeg;
mod serve;

use ok_br2_converter::{Converter, Disc, Options, Sink, cue_data_track, json::Json, open_image};
use std::path::{Path, PathBuf};
use std::time::Instant;

struct DirSink(PathBuf);

impl Sink for DirSink {
    fn put(&mut self, path: &str, data: &[u8]) -> Result<(), String> {
        if path.split('/').any(|c| c == ".." || c.is_empty()) {
            return Err(format!("bad output path {path}"));
        }
        let p = self.0.join(path);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(&p, data).map_err(|e| format!("{}: {e}", p.display()))
    }
}

/// Files an older version of this converter wrote that it no longer writes: the executable and the tables cut out of
/// it (now `engine/tables.okt`). A conversion into such a folder removes them.
const RETIRED: [&str; 2] = ["engine/exe.bin", "engine/tables.bin"];

/// Removes the files of `retired` that the folder `out` holds (each logged); Err when one cannot be removed.
fn remove_retired(out: &Path, retired: &[&str]) -> Result<Vec<String>, String> {
    let mut removed = Vec::new();
    for r in retired {
        let p = out.join(r);
        if p.is_file() {
            std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            eprintln!("removed {} (an older version's; no longer written)", p.display());
            removed.push(r.to_string());
        }
    }
    Ok(removed)
}

fn usage() -> ! {
    eprintln!("usage:\n  okconv-br2 convert IMAGE OUT [--viewer] [--no-video] [--threads N]\n  okconv-br2 serve DIR [--port N]\n  okconv-br2 check DIR");
    std::process::exit(2)
}

fn open_disc(image: &Path) -> Result<Disc, String> {
    let ext = image.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let reader = if ext == "cue" {
        let text = std::fs::read_to_string(image).map_err(|e| format!("{}: {e}", image.display()))?;
        let (file, offset) = cue_data_track(&text).map_err(|e| e.to_string())?;
        let bin = image.parent().unwrap_or(Path::new(".")).join(file);
        let f = std::fs::File::open(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
        Box::new(ok_br2_converter::FileImage::raw(std::io::BufReader::new(f), offset).map_err(|e| e.to_string())?)
    } else {
        let f = std::fs::File::open(image).map_err(|e| format!("{}: {e}", image.display()))?;
        open_image(std::io::BufReader::new(f)).map_err(|e| e.to_string())?
    };
    Disc::open(reader).map_err(|e| e.to_string())
}

fn convert(args: &[String]) -> Result<(), String> {
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let mut threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8);
    if let Some(i) = args.iter().position(|a| a == "--threads") {
        threads = args.get(i + 1).and_then(|n| n.parse().ok()).unwrap_or_else(|| usage());
    }
    let pos: Vec<&String> = pos.into_iter().filter(|a| args.iter().position(|x| x == "--threads").is_none_or(|i| args.get(i + 1) != Some(a))).collect();
    if pos.len() != 2 {
        usage();
    }
    let viewer = args.iter().any(|a| a == "--viewer");
    let no_video = args.iter().any(|a| a == "--no-video");
    let out = PathBuf::from(pos[1]);
    let t0 = Instant::now();
    let disc = open_disc(Path::new(pos[0]))?;
    if !disc.raw() {
        eprintln!("note: an ISO image holds no CD-XA audio: no music, movies without sound");
    }
    let mut conv = Converter::new(disc, Options { viewer, threads });
    if viewer {
        match ffmpeg::Ffmpeg::new(!no_video) {
            Some(f) => conv.set_extras(Box::new(f)),
            None => eprintln!("note: ffmpeg not found: no MP3 / MP4 previews for the viewer"),
        }
    }
    let mut sink = DirSink(out.clone());
    let mut last = String::new();
    let summary = conv
        .run(&mut sink, &mut |p| {
            let line = format!("[{:3}/{}] {}", p.done, p.total, p.step);
            if line != last {
                eprintln!("{line}");
                last = line;
            }
            true
        })
        .map_err(|e| e.to_string())?;
    remove_retired(&out, &RETIRED)?;
    let bytes: u64 = summary.files.values().map(|v| v.0).sum();
    eprintln!("done: {} files, {:.1} MB in {:.1} s -> {}", summary.files.len(), bytes as f64 / 1e6, t0.elapsed().as_secs_f64(), out.display());
    Ok(())
}

/// Every file of manifest.json `files` present with its size and checksum.
fn check(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(args.first().unwrap_or_else(|| usage()));
    let m = Json::parse(&std::fs::read_to_string(dir.join("manifest.json")).map_err(|e| e.to_string())?)?;
    let files = m.get("files").and_then(|f| f.as_arr()).ok_or("manifest.json has no `files` list")?;
    let mut bad = 0;
    for f in files {
        let (p, size, crc) = (
            f.get("path").and_then(Json::as_str).ok_or("a `files` entry without a path")?,
            f.get("size").and_then(Json::as_i64).unwrap_or(-1),
            f.get("crc32").and_then(Json::as_i64).unwrap_or(-1),
        );
        match std::fs::read(dir.join(p)) {
            Ok(d) if d.len() as i64 == size && ok_br2_converter::crc32(&d) as i64 == crc => {}
            Ok(_) => {
                println!("CHANGED {p}");
                bad += 1;
            }
            Err(_) => {
                println!("MISSING {p}");
                bad += 1;
            }
        }
    }
    println!("{} files, {} bad", files.len(), bad);
    if bad == 0 { Ok(()) } else { Err("check failed".into()) }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("convert") => convert(&args[1..]),
        Some("serve") => serve::serve(&args[1..]),
        Some("check") => check(&args[1..]),
        _ => usage(),
    };
    if let Err(e) = r {
        eprintln!("okconv-br2: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A conversion into a folder of an older version removes its retired files (the executable and the tables cut
    /// out of it), and nothing else; a folder without them is left as it is.
    #[test]
    fn retired_files_removed() {
        let dir = std::env::temp_dir().join(format!("okconv_br2_retired_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("engine")).unwrap();
        for f in ["engine/exe.bin", "engine/tables.bin", "engine/tables.okt"] {
            std::fs::write(dir.join(f), f).unwrap();
        }
        assert_eq!(remove_retired(&dir, &RETIRED).unwrap(), ["engine/exe.bin", "engine/tables.bin"]);
        assert!(!dir.join("engine/exe.bin").exists() && !dir.join("engine/tables.bin").exists());
        assert!(dir.join("engine/tables.okt").exists());
        assert!(remove_retired(&dir, &RETIRED).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
