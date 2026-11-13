//! Readable text from the executable SCUS_944.24, grouped by use, and the sound-test
//! names of the sound banks.

use crate::disc::Entry;
use crate::json::{Json, Obj};

/// file offset → RAM: addr = LOAD + offset (the PS-X EXE body starts at 0x800)
pub const LOAD: u32 = 0x8011_E000;

const GROUPS: [(&str, &str, &str); 8] = [
    ("Main menu", "OPTIONS", "ARCADE"),
    ("Stage select", "RANDOM", "CITY"),
    ("Story titles", "REINCARNATION", "BACK TO THE BATTLEFIELD"),
    ("Fight announcer & HUD", "STAGE", "PLEASE WAIT"),
    ("Debug menu (hidden)", "  <<< DEBUG MENU >>>", "BGM     "),
    ("Game modes & options", "PRACTICE", "CONSOLE"),
    ("Sound test", "[STAGE8]", "SE PLAYER"),
    ("Movies & memory card", "ENDING2", "COMPLETED !"),
];

pub struct Item {
    pub group: String,
    pub addr: String,
    pub text: String,
}

/// `str.strip()` for printable ASCII: only spaces can be stripped.
fn strip(s: &str) -> &str {
    s.trim_matches(' ')
}

/// Every human-readable string of the EXE (`gametext.extract`).
pub fn extract(exe: &[u8]) -> Vec<Item> {
    // runs of [\x20-\x7e]{3,}
    let mut found: Vec<(usize, String)> = Vec::new();
    let mut i = 0;
    while i < exe.len() {
        if (0x20..=0x7E).contains(&exe[i]) {
            let s = i;
            while i < exe.len() && (0x20..=0x7E).contains(&exe[i]) {
                i += 1;
            }
            if i - s >= 3 {
                let t = String::from_utf8(exe[s..i].to_vec()).unwrap();
                let alpha = t.chars().filter(|c| c.is_ascii_alphabetic()).count();
                if alpha >= 3 && !t.contains(['\\', '^', '`', '{', '|', '}', '~']) {
                    found.push((s, t));
                }
            }
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    let mut used = std::collections::HashSet::new();
    let mut pos = 0;
    for (label, first, last) in GROUPS {
        let Some(a) = (pos..found.len()).find(|&i| found[i].1 == first) else { continue };
        let b = (a..found.len()).find(|&i| found[i].1 == last).unwrap_or(a);
        pos = b + 1;
        for (o, s) in &found[a..=b] {
            out.push(Item { group: label.into(), addr: format!("0x{:08X}", LOAD + *o as u32), text: strip(s).into() });
            used.insert(*o);
        }
    }
    for (o, s) in &found {
        let t = strip(s);
        let n = t.chars().count();
        let good = t.chars().filter(|c| c.is_ascii_alphabetic() || *c == ' ').count();
        if !used.contains(o) && n >= 5 && good as f64 >= 0.8 * n as f64 {
            out.push(Item { group: "Other strings".into(), addr: format!("0x{:08X}", LOAD + *o as u32), text: t.into() });
        }
    }
    out
}

pub fn items_json(items: &[Item]) -> Json {
    Json::Arr(
        items.iter().map(|it| Json::Obj(Obj::new().with("group", it.group.as_str()).with("addr", it.addr.as_str()).with("text", it.text.as_str()))).collect(),
    )
}

pub fn items_txt(items: &[Item]) -> String {
    let mut s = String::new();
    let mut group: Option<&str> = None;
    for it in items {
        if group != Some(&it.group) {
            group = Some(&it.group);
            s.push_str(&format!("\n== {} ==\n", it.group));
        }
        s.push_str(&format!("{}  {}\n", it.addr, it.text));
    }
    s
}

/// The sound-test names of the .SND banks (table 0x801B3E44 and the SE player's id table): disc path → name.
pub fn sound_bank_names(exe: &[u8], files: &[(String, Entry)]) -> std::collections::HashMap<String, String> {
    let mut names = std::collections::HashMap::new();
    let base = LOAD as usize;
    let rd = |o: usize| exe.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    let mut labels = Vec::new();
    for k in 0..36 {
        let Some(p) = rd(0x801B_3E44 - base + 4 * k) else { return names };
        let Some(o) = p.checked_sub(base) else { return names };
        let Some(end) = exe.get(o..).and_then(|s| s.iter().position(|&b| b == 0)) else { return names };
        let Ok(l) = String::from_utf8(exe[o..o + end].to_vec()) else { return names };
        labels.push(l);
    }
    let mut ids: Vec<usize> = vec![137, 136, 138, 139, 140, 141];
    for c in 0..11 {
        for k in [0, 1] {
            ids.push(196 + 8 * c + k);
        }
    }
    ids.extend(176..184);
    let tab = 0x801B_593C - base;
    let bcd = |x: u8| ((x >> 4) * 10 + (x & 15)) as i64;
    for (label, i) in labels.iter().zip(ids) {
        let Some(p) = rd(tab + 8 * i) else { break };
        let o = p.wrapping_sub(base);
        let Some(m) = exe.get(o..o + 3) else { break };
        let lba = (bcd(m[0]) * 60 + bcd(m[1])) * 75 + bcd(m[2]) - 150;
        for (name, e) in files {
            if e.lba as i64 == lba {
                names.insert(name.clone(), label.trim_matches(['[', ']']).to_string());
            }
        }
    }
    names
}
