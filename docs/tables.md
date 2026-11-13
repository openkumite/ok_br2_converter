# `engine/tables.okt`: the game's static tables

The converted data holds no copy of the game's executable. The converter cuts the static tables a runtime needs out of
it and writes them, each under a name, into `engine/tables.okt` (data format 3; `src/okt.rs` writes it,
`src/gamedata.rs` `TABLES` lists them).

## File

All numbers little-endian. Table names are unique within a file.

```text
"BR2TABLE" | u32 version = 1 | u32 count |
count × { u8 name_len, name (UTF-8), u8 elem, u16 rec_size, u32 count, u32 origin, u32 byte_len, bytes, pad to 4 }
```

| `elem` | element | size |
|---|---|---|
| 0 | blob (raw bytes; `count` = `byte_len`) | 1 |
| 1 | u8 | 1 |
| 2 | i16 | 2 |
| 3 | u16 | 2 |
| 4 | u32 (e.g. `count` 1: a scalar) | 4 |
| 5 | record | `rec_size` |

`byte_len` = `count` × the element's size. Padding (zeros) brings the next table to a multiple of 4 from the file's
start. `origin` is the address the table's first byte has in the address space the data's own links use: a table
holding addresses of records (the effect tracks, the timelines) points into the origins of the tables that hold those
records, so a reader resolves an address by finding the table whose `[origin, origin + byte_len)` holds it. Tables
never overlap. The bytes are the executable's, unchanged, but for the text table `text.strings` (below).

An origin is a RAM address of the game (0x80000000 and up), with these exceptions, which a reader looks up by name only:
the text table `text.strings` (origin 0).

The executable itself is not part of the converted data: the converter writes no `engine/exe.bin` or
`engine/tables.bin` (since format 2). Folders made by an older version may still hold them: `okconv-br2 convert` into
such a folder removes them.

## Tables (Bloody Roar II)

| name | element × count | what |
|---|---|---|
| `math.random_bytes` | u8 × 256 | the random byte table (the CPU, the effects) |
| `font.metrics` | blob 0x224 | the fonts' metrics: the advance widths of fonts 0..4, font 3's glyph cells (layout below) |
| `stage.light` | record 0x44 × 13 | per stage ST0..STC: the fighters' light (directions, colours, ambient) |
| `camera.presets` | record 8 × 8 | the TV camera's eye positions (vectors x, y, z, pad) |
| `camera.random` | record 8 × 8 | the TV camera's random eye positions (vectors x, y, z, pad; 0x801306D4) |
| `camera.heights` | i16 × 4 | the TV camera's height per mode |
| `camera.throw` | record 4 × 8 | the throw camera's angles (angle, flag) |
| `camera.track` | i16 × 4 | the tracking shot's yaw offsets |
| `stage.walls` | record 16 × 13 | per stage: the walls' model and hit points |
| `stage.backdrop` | record 26 × 13 | per stage: the backdrop (sky) parameters |
| `math.atan` | i16 × 0x401 | arctangent of a ratio (0..0x400) |
| `math.tan` | i16 × 0x3AF | tangent (the fighters' light) |
| `math.sin` | i16 × 0x401 | sine over a quarter turn |
| `screen.title_widths` | u32 × 24 | the screen titles' banner widths, entry for entry with the texts `screen_title.<n>` |
| `stage.grid_ring` | record 8 × 13 | per stage: the ring the drive grid is laid over (kind, drop, half x, half z) |
| `stage.ring` | record 8 × 13 | per stage: the ring (kind, drop, half x, half z) |
| `stage.ring_outer` | record 8 × 13 | per stage: the outer ring |
| `stage.drive_grid` | u32 × 13 | per stage: the drive grid's height |
| `stage.shadow_floor` | record 8 × 10 | per stage ST0..ST9: the shadows' floor (shape, height off the ring, half x, half z) |
| `fx.hit_sprites` | blob 0x414 | the hit sparks' sprites: sizes, CLUTs, the stage selector, texture pages, UVs, the keyframe tracks and their pointer table, the colours (linked) |
| `fxvm.static_block` | blob 0x2C | the effect script VM's static block (the UV animator's deltas and script), mapped at its origin |
| `fxvm.timeline_records` | blob 0x50 | the emitters' timeline records |
| `fxvm.timelines` | u32 × 4 | the four timelines an effect op picks from (addresses into `fxvm.timeline_records`) |
| `fxvm.emitter_timelines` | blob 0x28 | the two timelines the move scripts start emitters with |
| `fx.lens_flare` | blob 0x64 | the lens flare of City and Lab: the models, their depths, the positions (two rows of five vectors) |
| `fx.colour_flash` | blob 0xD0 | the colour flash patterns (a count, then that many RGB triples of i16) and their pointer table (linked) |
| `fx.move_effects` | blob 0x46C | the moves' effects: claw and kick trails, rays, streaks, the sparkle schedules and their pointer table (linked), the rays' speeds and widths, the streaks' CLUTs; it ends at 0x801B20D8, before the front end's data |
| `menu.mode_half` | u8 × 7 | the mode menu's half widths, item by item (the text list `modes`) |
| `select.normal_order` | u8 × 11 | the NORMAL character select's face order |
| `vs.announcer_delays` | record 2 × 11 | the VS screen's announcer pauses after the name and after the beast word, per character |
| `hud.frame` | record 4 × 8 | the HUD frames' pieces: texture rectangles (u, v, w, h) of the corners, edges and sides |
| `select.neighbours` | record 4 × 11 | the select cursor's neighbours (up, right, down, left) |
| `select.faces` | record 4 × 11 | the select's face cells |
| `select.plates` | record 2 × 11 | the name plates' x |
| `select.story_beast_x` | u8 × 11 | the STORY select's beast picture x |
| `select.stage_half` | u8 × 9 | the stage names' half widths |
| `select.story_title_x` | u8 × 11 | the STORY select's title x |
| `select.story_name_x` | u8 × 11 | the STORY select's name line x |
| `sound.menu_volumes` | record 6 × 6 | the ending bank's sound definitions (priority, pan, voice class, volume) |
| `movie.records` | record 28 × 4 | the movies (the last frame at +8) |
| `input.default_buttons` | u16 × 8 | the default button configuration |
| `progress.rank` | record 16 × 8 | the arcade rank table (DIFFICULTY × fight) |
| `menu.cc_values` | u16 × 10 | the controller config's values |
| `story.fight_count` | u8 × 11 | STORY mode: the fights per character |
| `story.stages` | record 8 × 11 | STORY mode: the stages, 8 a character |
| `story.opponents` | record 8 × 11 | STORY mode: the opponents, 8 a character |
| `story.script` | record 25 × 11 | STORY mode: the page script |
| `stage.object_slots` | record 23 × 13 | per stage: the object slots the stage's drawing fills |
| `hud.data` | blob 0x3B8 | the HUD: the names' x, the sprites' texture rectangles (u, v, w, h) and their animations |
| `hud.popup` | u16 × 20 | the BEAST pop-up: its palette cycle ((row, frames) pairs, ended by 0xFF00, a pad), then its 4 CLUT ids |
| `ai.cpu` | u8 × 0xA0 | the CPU's threshold tables |
| `ai.recover_pads` | u16 × 8 | the CPU's recovery pads |
| `math.sqrt` | i16 × 192 | square roots |
| `math.rsqrt` | i16 × 192 | reciprocal square roots |
| `text.strings` | blob | the game's text (below) |

A reader's out-of-range read (past a table's end, or an address no table holds) gives 0.

`font.metrics`, by offset from its origin 0x8011ED80 (the text drawer 0x8012D258.. picks a font by its jump table
0x8011F158; one byte per entry):

| offset | what |
|---|---|
| +0x000 | font 0: 96 advance widths, from the space on |
| +0x060 | font 1: 96 advance widths, from the space on |
| +0x0C0 | font 2: the advance widths from `A` on; +0x0DB its space's width |
| +0x0F4 | font 3: the advance widths by letter; +0x10F its space's width; +0x124 the glyph cells' u, +0x154 their v |
| +0x184 | font 4: the advance widths; +0x1E4 its space's width; the table ends at +0x223 |

Fonts 0, 1, 2 and 4 have no cell table here: their glyph cells come from the drawer's code, not from a table.

## Text (`text.strings`)

The text the game draws, as a blob of entries `key NUL text NUL`, origin 0 (it holds no addresses). The converter
reads each text as the executable's C string and cuts its trailing spaces and line ends (a reader places the text
itself), but for the list `censor`, whose 3-letter words the game compares spaces included; the text is otherwise
unchanged (ASCII; `%s`, `%u` and `%d` mark the values a reader fills in). Keys (`src/gamedata.rs` `TEXT_LISTS`,
`TEXT`): a list's entries are `<list>.<n>` from 0 (`names`: each character's name, then its beast; `stages`, `modes`,
`screen_title`, `story_title`, `displays`, `cc_names`, `cc_icons`, `movies`, `censor`, `ordinal`, `se_banks`); the single texts have
keys made of their words (`you_win`, `press_start`; a `_n` marks a filled-in value, `_ask` and `_done` the question and
the statement of the same text). A reader looks texts up by key and must not depend on the entries' order.
