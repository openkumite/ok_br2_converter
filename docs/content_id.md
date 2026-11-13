# The content id

`manifest.json` carries `content_id`, a number of 1..=4294967295 (u32) that names the converted data a game's
simulation runs on: two conversions with the same id hold the same simulation data, so a program can compare ids to
tell, for example, whether two players or a recording and a player have the same data.

## Formula

`src/convert.rs` `content_id`: CRC-32 (IEEE, as the manifest's file CRCs) of the text made of one line

```text
<path>:<crc32>\n
```

per written file that `in_content` covers, in byte order of the paths (the manifest's `files` order), with `<crc32>` the
file's CRC-32 as 8 lower-case hex digits; a CRC of 0 becomes 1 (0 is left free to mean "none").

The covered files are the ones the simulation reads:

| covered | why |
|---|---|
| `engine/**` | the disc's files the logic parses (ACT / ANI, INIT.DAT, effects, stages, sound banks' tables, …) and `engine/tables.okt` |
| `characters/**`, `stages/**` | the fighters' skeletons and the stages' models, ordering tables and wall debris |
| `video/*.bsf` | the movies' frames: their counts time the front end |

Not covered: the music and the movies' sound (`audio/**`, `video/*.xa`; an ISO image has none, so a conversion of the
raw image and one of an ISO share the id when their covered files are the same) and the viewer's files.

The conversion is deterministic, so two conversions of the same disc by the same converter give the same id
(`tests/manifest.rs` `content_id_is_stable`, on BR2_DISC). A converter change that alters a covered file changes
the id.
