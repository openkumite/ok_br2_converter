# ok_br2_converter

Converts your own Bloody Roar II disc (PlayStation, USA, SCUS-94424) into game data files: the fighters and arenas as
glTF models with every animation, the game's raw runtime files and the static tables cut out of its executable, the
soundtrack and the movies. Optionally it adds pictures, data listings and a three.js viewer page. The output is
deterministic: the same disc gives the same bytes on every platform. Runs on the command line, and as a library in
native and WebAssembly programs.

> **Unofficial.** ok_br2_converter is not affiliated with, endorsed or sponsored by Konami or Sony Interactive
> Entertainment. Bloody Roar is a trademark of Konami Digital Entertainment (the series was developed by Hudson Soft,
> now part of Konami); PlayStation is a trademark of Sony Interactive Entertainment Inc. This repository contains no
> game data: you need your own legally obtained copy of the disc, which the converter reads on your computer. Do not
> ask for, or share, disc images or converted files.

## Quick start

You need [Rust](https://rustup.rs) and an image of the disc: BIN/CUE (raw 2352-byte sectors), CHD or ISO.

```sh
cargo build --release
target/release/okconv-br2 convert "Bloody Roar II (USA).cue" out            # about 5 s on 8 threads
target/release/okconv-br2 convert "Bloody Roar II (USA).cue" out --viewer   # + pictures, listings, viewer page
target/release/okconv-br2 serve out                                          # the viewer at http://localhost:8000
```

## Commands

| command | what |
|---|---|
| `okconv-br2 convert IMAGE OUT [--viewer] [--no-video] [--threads N]` | converts the disc image into the folder OUT |
| `okconv-br2 serve DIR [--port N]` | serves the viewer of a `--viewer` conversion (HTTP Range requests, so the movies can seek) |
| `okconv-br2 check DIR` | checks every file of a conversion against the sizes and CRC-32s in its `manifest.json` |

`--viewer` uses the system `ffmpeg`, if present, for MP3 previews of the music and MP4 versions of the movies
(`--no-video` skips the movies). The library itself never runs external programs. A conversion into a folder made by
an older version removes the files that version wrote and this one no longer does.

## Disc images

| image | |
|---|---|
| `.cue` + `.bin` | raw 2352-byte sectors: everything |
| `.chd` | MAME's compressed format (feature `chd`, on by default): everything |
| `.iso` | 2048-byte sectors: the XA music and the movies' sound are not on it, so the conversion skips them (the movies' frames are converted) and `manifest.json` says so (`xa: false`) |

Another disc is refused (`DiscError::WrongDisc`, naming what was found).

## Library

```rust
use ok_br2_converter::{Converter, Disc, MemorySink, Options, open_image};

let disc = Disc::open(open_image(std::fs::File::open("game.chd")?)?)?;   // or FileImage::raw(bin, offset) from a CUE
let mut conv = Converter::new(disc, Options { viewer: false, threads: 1 });
let mut sink = MemorySink::default();                                    // or your own `Sink`
while !conv.step(&mut sink)? {                                           // one step at a time (e.g. per frame) ...
    let p = conv.progress();                                             // ... with progress in between
}
// or: conv.run(&mut sink, &mut |p| keep_going)  (threads with the `threads` feature)
```

The library uses no file system: input is any `Read + Seek`, a `ReadAt` (`open_read_at`; `open_cue` takes an opener for
the BIN a CUE names) or your own `SectorReader`; output is any `Sink`. The disc image readers are usable on their own:
`open_cue_all` also opens a CUE's AUDIO tracks (each from its INDEX 01 to the next track of its FILE or the FILE's end;
a FILE that cannot be opened is left out and named in `CueImage::missing`), and a CHD's AUDIO tracks are read too
(`SectorReader::audio_tracks` / `read_audio`: 16-bit little-endian stereo, the CHD's big-endian samples swapped; tracks
padded to 4 frames, a `V…` pregap skipped). It builds for `wasm32-unknown-unknown` and Android. The feature `threads`
(on by default) lets `run` use `Options::threads` worker threads (not on wasm32).

## Output

| path | what |
|---|---|
| `manifest.json` | the index: fighters, arenas, music, …; `format` (data format, 3), `source` (`bin` / `chd` / `iso`), `xa` (music and movie sound present), `content_id` (the simulation data's id: [docs/content_id.md](docs/content_id.md)), `files` (`[{path, size, crc32}]`, CRC-32 IEEE, every file but `manifest.json`), `converter` (details) |
| `characters/<CODE>_<n>P_<human\|beast>.glb` | the fighters, skinned, with every animation |
| `stages/*.glb` | the arenas, in several variants (ordering-table units, split objects, semi-transparent parts, wall debris) |
| `engine/**` | the runtime data: the disc's raw game files (ANI / ACT, sound banks, effects, …) and the index of what they hold |
| `engine/tables.okt` | the executable's static tables by name and the game's text ([docs/tables.md](docs/tables.md)); the executable itself is not copied |
| `audio/music/BR2_MC0x_chN.xa` | the soundtrack, one file per XA channel |
| `video/<NAME>.bsf`, `video/<NAME>.xa` | the movies: MDEC frames and their sound |
| `--viewer` only | `images/**`, `data/**`, `text/**`, `audio/sfx/**.wav`, `index.html`, MP3 / MP4 previews |

Floating-point numbers in the glTF and JSON files follow Python's arithmetic and `repr()` (signed zeros, `sum()`,
`round()`), the PNGs are zlib 1.2.12 level-9 deflate, and the animation math uses the pure-Rust `libm`, so every
target writes the same bytes.

### `.xa` (version 1)

The music and the movies' sound, kept as the disc's CD-XA ADPCM.

| offset | size | |
|---|---|---|
| 0 | 6 | `BR2XA\0` |
| 6 | 1 | version (1) |
| 7 | 1 | the XA subheader's coding byte (bit 0 stereo, bit 2 18.9 kHz else 37.8 kHz, bit 4 8-bit else 4-bit) |
| 8 | 4 | sector count N (LE) |
| 12 | 4 | 0 |
| 16 | N × 2304 | each audio sector's 18 sound groups of 128 bytes, in disc order, up to and including the end-of-file sector |

### `.bsf`

The movies' frames: `BRBS`, u16 width, u16 height, u32 frame count, then per frame u32 sector, u32 length and the
frame's MDEC bitstream (all little-endian).

## Development

`cargo fmt --check`, `cargo clippy --release --all-targets -- -D warnings` and `cargo test --release` run without the
disc. The tests that convert the real disc run when `BR2_DISC` names an image (`BR2_ISO` an ISO of it, too):
`BR2_DISC="Bloody Roar II (USA).cue" cargo test --release`.

## License

MIT ([LICENSE](LICENSE)). Third-party code and its licenses: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
