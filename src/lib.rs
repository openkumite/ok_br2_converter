//! Bloody Roar II (USA, SCUS-94424) disc image → game data files: the fighters and stages as glTF, the runtime data
//! (`engine/`: the disc's raw game files and the executable's tables), the music and the movies; optionally pictures,
//! listings and a viewer page. The output is deterministic: the same disc gives the same bytes on every target.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use ok_br2_converter::{Converter, Disc, MemorySink, Options, open_image};
//! let file = std::fs::File::open("Bloody Roar II (USA).bin")?;
//! let disc = Disc::open(open_image(file)?)?;
//! let mut sink = MemorySink::default();
//! let summary = Converter::new(disc, Options::default()).run(&mut sink, &mut |p| {
//!     println!("{}/{} {}", p.done, p.total, p.step);
//!     true
//! })?;
//! # Ok(()) }
//! ```

pub mod audio;
#[cfg(feature = "chd")]
pub mod chd_image;
pub mod convert;
pub mod disc;
pub mod gamedata;
pub mod glb;
pub mod hmd;
pub mod images;
pub mod json;
pub mod lz;
pub mod model;
pub mod okt;
pub mod png;
pub mod py;
pub mod stages;
pub mod text;
pub mod vram;
pub mod zlib;

pub use convert::{Converter, Error, Extras, FORMAT_VERSION, MemorySink, Options, Progress, Sink, Summary, content_id, in_content};
pub use disc::{
    AudioTrack, CueImage, Disc, DiscError, FileImage, MaybeSend, ReadAt, ReadAtStream, SectorReader, cue_data_track, memory_image, open_cue, open_cue_all,
    open_image, open_read_at, parse_cue,
};
pub use zlib::crc32;

/// The three.js viewer page written with `Options::viewer`.
pub const VIEWER_HTML: &str = include_str!("../viewer/index.html");
