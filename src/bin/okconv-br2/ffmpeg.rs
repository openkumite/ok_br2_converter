//! The viewer's previews through the system ffmpeg: music as MP3, the FMVs as H.264/AAC MP4 with a poster
//! frame. Not part of the game data.

use ok_br2_converter::Extras;
use ok_br2_converter::json::{Json, Obj};
use std::path::PathBuf;
use std::process::Command;

pub struct Ffmpeg {
    tmp: PathBuf,
    video: bool,
}

const TITLES: [(&str, &str); 4] =
    [("USAOP", "Opening (USA)"), ("BR", "CG character movie"), ("ED1", "Staff credits (version 1)"), ("ED2", "Staff credits (version 2, US team)")];

fn found(tool: &str) -> bool {
    Command::new(tool).arg("-version").output().is_ok_and(|o| o.status.success())
}

impl Ffmpeg {
    pub fn new(video: bool) -> Option<Ffmpeg> {
        if !found("ffmpeg") {
            return None;
        }
        let tmp = std::env::temp_dir().join(format!("okconv-br2-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).ok()?;
        Some(Ffmpeg { tmp, video: video && found("ffprobe") })
    }
}

impl Drop for Ffmpeg {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

fn run(args: &[&str]) -> bool {
    Command::new(args[0]).args(&args[1..]).status().is_ok_and(|s| s.success())
}

fn output(args: &[&str]) -> String {
    Command::new(args[0]).args(&args[1..]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

impl Extras for Ffmpeg {
    fn music_preview(&mut self, base: &str, wav: &[u8]) -> Option<(String, Vec<u8>)> {
        let w = self.tmp.join(format!("{base}.wav"));
        let m = self.tmp.join(format!("{base}.mp3"));
        std::fs::write(&w, wav).ok()?;
        let ok = run(&["ffmpeg", "-v", "error", "-y", "-i", w.to_str()?, "-b:a", "192k", m.to_str()?]);
        let _ = std::fs::remove_file(&w);
        let data = if ok { std::fs::read(&m).ok() } else { None };
        let _ = std::fs::remove_file(&m);
        Some((format!("audio/music/{base}.mp3"), data?))
    }

    fn video(&mut self, base: &str, source: &str, raw: &[u8]) -> Option<(Vec<(String, Vec<u8>)>, Json)> {
        if !self.video {
            return None;
        }
        let s = self.tmp.join(format!("{base}.str"));
        let mp4 = self.tmp.join(format!("{base}.mp4"));
        let jpg = self.tmp.join(format!("{base}.jpg"));
        std::fs::write(&s, raw).ok()?;
        let (sp, mp, jp) = (s.to_str()?, mp4.to_str()?, jpg.to_str()?);
        // STR streams at double CD speed (150 sectors/s): the length is known from the sector count; ffmpeg assumes
        // 15 fps, but BR.STR is 30 fps
        let secs = (raw.len() / 2352) as f64 / 150.0;
        let packets =
            output(&["ffprobe", "-v", "error", "-select_streams", "v:0", "-count_packets", "-show_entries", "stream=nb_read_packets", "-of", "csv=p=0", sp]);
        let fps = packets.parse::<f64>().map(|p| ((p / secs).round_ties_even() as i64).max(1)).unwrap_or(15);
        let fr = fps.to_string();
        let ok = run(&[
            "ffmpeg",
            "-v",
            "error",
            "-y",
            "-r",
            &fr,
            "-i",
            sp,
            "-c:v",
            "libx264",
            "-crf",
            "18",
            "-preset",
            "slow",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "160k",
            "-movflags",
            "+faststart",
            mp,
        ]);
        let _ = std::fs::remove_file(&s);
        if !ok {
            return None;
        }
        let dur: f64 = output(&["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", mp]).parse().unwrap_or(0.0);
        let ss = format!("{:.2}", dur * 0.25);
        run(&["ffmpeg", "-v", "error", "-y", "-ss", &ss, "-i", mp, "-vf", "thumbnail=90", "-frames:v", "1", "-q:v", "3", jp]);
        let mut files = vec![(format!("video/{base}.mp4"), std::fs::read(&mp4).ok()?)];
        if let Ok(j) = std::fs::read(&jpg) {
            files.push((format!("video/{base}.jpg"), j));
        }
        let _ = std::fs::remove_file(&mp4);
        let _ = std::fs::remove_file(&jpg);
        let title = TITLES.iter().find(|t| t.0 == base).map(|t| t.1).unwrap_or(base);
        let e = Obj::new()
            .with("file", format!("video/{base}.mp4"))
            .with("poster", format!("video/{base}.jpg"))
            .with("fps", fps)
            .with("title", title)
            .with("source", source)
            .with("duration", ok_br2_converter::py::round(dur, 1));
        Some((files, Json::Obj(e)))
    }
}
