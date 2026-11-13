//! Pictures for the viewer (the standalone TIMs and the 2D screens): endings, story art and text, portraits, menu /
//! HUD sprite sheets, the effect sheet and its sprites, VS backgrounds.

use crate::json::{Json, Obj};
use crate::lz::read_container;
use crate::png::png_bytes;
use crate::vram::{page_8bit, page_gray4, rgba, stitch_2x2, tim_to_rgba};
use std::collections::{BTreeMap, HashSet};

pub type Files = BTreeMap<String, Vec<u8>>;
pub type Out = Vec<(String, Vec<u8>)>;

pub const STORY_CHARS: [(&str, &str); 11] = [
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

pub const STORY_TITLES: [(&str, &str); 11] = [
    ("YUG", "Back to the Battlefield"),
    ("ALI", "Love for the Wounded"),
    ("LON", "Denying His Fate"),
    ("ULI", "Outstanding Adventure of a Kitten"),
    ("MAR", "Young Mercenary"),
    ("BUS", "Appalling Science"),
    ("BAK", "Over the Painful Memories"),
    ("JEN", "Observer's Eye"),
    ("STU", "Return of the Bug-Guy"),
    ("GAD", "2 Days of the Old Lion"),
    ("RON", "Reincarnation"),
];

pub fn lookup(t: &[(&'static str, &'static str)], k: &str) -> Option<&'static str> {
    t.iter().find(|e| e.0 == k).map(|e| e.1)
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}
fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

/// The disc path's file name without its 4-character extension (`os.path.basename(name)[:-4]`).
pub fn stem(name: &str) -> &str {
    let b = name.rsplit('/').next().unwrap_or(name);
    &b[..b.len().saturating_sub(4)]
}

fn image_group(folder: &str) -> String {
    if let Some(c) = lookup(&STORY_CHARS, folder) {
        return format!("Endings · {c}");
    }
    match folder {
        "STY" => "Story pictures".into(),
        "CMN" => "Title & menus".into(),
        "EFT" => "Effects".into(),
        f => f.into(),
    }
}

fn entry(group: &str, file: &str, source: &str, index: Json) -> Json {
    Json::Obj(Obj::new().with("group", group).with("file", file).with("source", source).with("index", index))
}

/// The standalone 0x10 TIMs of every container (`convert.main`'s picture loop).
pub fn pictures(files: &Files, out: &mut Out) -> Vec<Json> {
    let mut list = Vec::new();
    for (name, data) in files {
        if !name.ends_with(".DAT") {
            continue;
        }
        let group = name.trim_matches('/').split('/').next().unwrap_or("");
        let base = stem(name);
        let Ok(ents) = read_container(data) else { continue };
        let tims: Vec<(usize, &Vec<u8>)> =
            ents.iter().enumerate().filter_map(|(i, e)| e.as_ref().filter(|e| e.len() >= 20 && u32_at(e, 0) == 0x10).map(|e| (i, e))).collect();
        // title screen and ending art: four 256x240 tiles forming one 512x480 picture
        if (base == "USATITLE" || base.ends_with("CON")) && tims.len() == 4 {
            let r: Vec<Option<(usize, usize, Vec<u8>)>> = tims.iter().map(|(_, e)| tim_to_rgba(e)).collect();
            if r.iter().all(|x| x.as_ref().is_some_and(|x| (x.0, x.1) == (256, 240))) {
                let t: Vec<&[u8]> = r.iter().map(|x| x.as_ref().unwrap().2.as_slice()).collect();
                let rel = format!("images/{group}/{base}.png");
                out.push((rel.clone(), png_bytes(512, 480, &stitch_2x2(&[t[0], t[1], t[2], t[3]], 256, 240))));
                list.push(entry(&image_group(group), &rel, name, "all".into()));
                continue;
            }
        }
        for (i, e) in tims {
            let Some((w, h, px)) = tim_to_rgba(e) else { continue };
            let rel = format!("images/{group}/{base}_{i:02}.png");
            out.push((rel.clone(), png_bytes(w, h, &px)));
            list.push(entry(&image_group(group), &rel, name, i.into()));
        }
    }
    list
}

/// Bounding boxes of separate shapes on a 4-bit sheet (index 0 = empty), merging shapes closer than `gap`.
fn cut_sprites(px: &[u8], w: usize, h: usize, gap: i64, min_px: usize) -> Vec<(usize, usize, usize, usize)> {
    let idx: Vec<u8> = px.iter().flat_map(|&b| [b & 15, b >> 4]).collect();
    let mut seen = vec![false; w * h];
    let mut boxes = Vec::new();
    for start in 0..w * h {
        if idx[start] == 0 || seen[start] {
            continue;
        }
        let mut stack = vec![start];
        let (mut x0, mut y0, mut x1, mut y1, mut n) = (w, h, 0, 0, 0);
        seen[start] = true;
        while let Some(p) = stack.pop() {
            let (x, y) = (p % w, p / w);
            (x0, y0, x1, y1, n) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y), n + 1);
            for dy in -gap..=gap {
                for dx in -gap..=gap {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if 0 <= nx && nx < w as i64 && 0 <= ny && ny < h as i64 {
                        let q = ny as usize * w + nx as usize;
                        if idx[q] != 0 && !seen[q] {
                            seen[q] = true;
                            stack.push(q);
                        }
                    }
                }
            }
        }
        if n >= min_px {
            boxes.push((x0, y0, x1, y1));
        }
    }
    boxes.sort_by_key(|b| (b.1 / 16, b.0));
    boxes
}

/// `export_2d`: story text pages, portraits, menu / HUD sprite sheets, the hit-effect sheet, VS backgrounds.
pub fn export_2d(files: &Files, out: &mut Out) -> Vec<Json> {
    let mut list = Vec::new();
    let mut hashes: HashSet<Vec<u8>> = HashSet::new();
    let mut save = |out: &mut Out, group: &str, name: &str, w: usize, h: usize, px: Vec<u8>, source: &str, index: usize| {
        if hashes.contains(&px) {
            return;
        }
        let rel = format!("images/{}/{}.png", group.replace(' ', "_").replace('·', "").replace("__", "_"), name);
        out.push((rel.clone(), png_bytes(w, h, &px)));
        hashes.insert(px);
        list.push(entry(group, &rel, source, index.into()));
    };
    let entries = |name: &str| -> Vec<Option<Vec<u8>>> { files.get(name).and_then(|d| read_container(d).ok()).unwrap_or_default() };
    let magic = |e: &Option<Vec<u8>>| -> Option<u32> { e.as_ref().filter(|e| e.len() >= 20).map(|e| u32_at(e, 0)) };
    // story text: /STY/<CHAR>/<CHAR>NN.DAT
    for name in files.keys().filter(|n| n.starts_with("/STY/") && !n.contains("/PIC/")) {
        let code = name.split('/').nth(2).unwrap_or("");
        for (i, e) in entries(name).iter().enumerate() {
            if magic(e) == Some(0x12) && u32_at(e.as_ref().unwrap(), 4) & 3 == 0 {
                let (w, h, px) = page_gray4(e.as_ref().unwrap(), true);
                let group = format!("Story text · {} — {}", lookup(&STORY_CHARS, code).unwrap_or(code), lookup(&STORY_TITLES, code).unwrap_or(""));
                save(out, &group, &format!("{}_{i:02}", stem(name)), w, h, px, name, i);
            }
        }
    }
    // portraits (4-bit grayscale pages)
    for (name, label) in [("/CMN/P00.DAT", "Portraits · beasts"), ("/CMN/P01.DAT", "Portraits")] {
        for (i, e) in entries(name).iter().enumerate() {
            if magic(e) == Some(0x12) {
                let (w, h, px) = page_gray4(e.as_ref().unwrap(), false);
                save(out, label, &format!("{}_{i:02}", stem(name)), w, h, px, name, i);
            }
        }
    }
    // the palette bank of the 8-bit menu sheets
    let mut bank: Option<Vec<u16>> = None;
    for name in ["/CMN/VS/VS.DAT", "/CMN/SEL00.DAT"] {
        for e in entries(name).iter() {
            if magic(e) == Some(0x13) {
                let e = e.as_ref().unwrap();
                bank = Some((0..4096).map(|i| u16_at(e, 20 + 2 * i)).collect());
                break;
            }
        }
        if bank.is_some() {
            break;
        }
    }
    let pal0: Option<Vec<u16>> = bank.map(|b| b[..256].to_vec());
    for name in ["/CMN/VS/VS.DAT", "/CMN/SEL00.DAT"] {
        for (i, e) in entries(name).iter().enumerate() {
            if magic(e) == Some(0x12) {
                let (w, h, px) = page_8bit(e.as_ref().unwrap(), pal0.as_deref());
                save(out, "Menu sprites", &format!("{}_{i:02}", stem(name)), w, h, px, name, i);
            }
        }
    }
    // INIT: fonts / HUD sprites (4-bit) and face icons (8-bit), grayscale
    for (i, e) in entries("/CMN/INIT.DAT").iter().enumerate() {
        if magic(e) == Some(0x12) {
            let e = e.as_ref().unwrap();
            let (w, h, px) = if u32_at(e, 4) & 3 == 0 { page_gray4(e, false) } else { page_8bit(e, None) };
            save(out, "HUD & fonts", &format!("INIT_{i:02}"), w, h, px, "/CMN/INIT.DAT", i);
        }
    }
    // the hit-effect sheet: one 4-bit page and two 16-palette CLUT blocks -> every colour variant
    let eft = entries("/EFT/EFT.DAT");
    let page = eft.iter().find(|e| magic(e) == Some(0x12)).map(|e| e.as_ref().unwrap().clone());
    if let Some(page) = &page {
        let (pw, ph) = (u16_at(page, 16) as usize, u16_at(page, 18) as usize);
        let px = &page[20..(20 + pw * ph * 2).min(page.len())];
        let mut k = 0;
        for e in &eft {
            if magic(e) != Some(0x11) {
                continue;
            }
            let e = e.as_ref().unwrap();
            let clut: Vec<u16> = (0..256).map(|i| u16_at(e, 20 + 2 * i)).collect();
            for row in 0..16 {
                let pal: Vec<[u8; 4]> = clut[row * 16..row * 16 + 16].iter().map(|&v| rgba(v)).collect();
                if !pal[1..].iter().any(|p| p[3] != 0) {
                    continue;
                }
                let img: Vec<u8> = px.iter().flat_map(|&b| [pal[(b & 15) as usize], pal[(b >> 4) as usize]]).flatten().collect();
                save(out, "Effects · hit sparks", &format!("EFT_palette{k:02}"), pw * 4, ph, img, "/EFT/EFT.DAT", k);
                k += 1;
            }
        }
        let sprites = cut_sprites(px, pw * 4, ph, 2, 12);
        let clut0 = eft.iter().find(|e| magic(e) == Some(0x11)).and_then(|e| e.as_ref());
        if let Some(clut0) = clut0 {
            // row 1: neutral white
            let pal: Vec<[u8; 4]> = (0..16).map(|i| rgba(u16_at(clut0, 20 + 32 + 2 * i))).collect();
            let full: Vec<[u8; 4]> = px.iter().flat_map(|&b| [pal[(b & 15) as usize], pal[(b >> 4) as usize]]).collect();
            let mut rects = Vec::new();
            for (k, &(x0, y0, x1, y1)) in sprites.iter().enumerate() {
                let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
                let mut img = Vec::with_capacity(4 * w * h);
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        img.extend_from_slice(&full[y * pw * 4 + x]);
                    }
                }
                save(out, "Effects · sprites", &format!("EFT_sprite{k:03}"), w, h, img, "/EFT/EFT.DAT", k);
                rects.push(Json::Obj(Obj::new().with("id", k).with("x", x0).with("y", y0).with("w", w).with("h", h)));
            }
            let js = Json::Obj(Obj::new().with("sheet", "EFT/EFT.DAT page 257 (256x256, 4-bit)").with("detected", true).with("sprites", rects));
            out.push(("images/Effects_sprites/sprite_rects.json".into(), js.indent(0).into_bytes()));
        }
    }
    // VS-screen backgrounds: standalone uncompressed TIMs
    for (name, data) in files.iter().filter(|(n, _)| n.starts_with("/CMN/VS/SB")) {
        if let Some((w, h, px)) = tim_to_rgba(data) {
            save(out, "VS backgrounds", stem(name), w, h, px, name, 0);
        }
    }
    list
}
