//! Game-logic data for the viewer: every .ANI section with its move scripts disassembled, the effect scripts (VM at
//! 0x801B0BD0) and emitter definitions, the VM's size tables; and the runtime tables: `engine/tables.okt`
//! (`TABLES`, written by `tables_okt`) and engine/index.json.

use crate::json::{Json, Obj};
use crate::okt::{self, Elem};
use crate::text::LOAD;

const EXE_LO: u32 = 0x8011_E800;
const EXE_HI: u32 = 0x801B_F000;

/// Instruction sizes per op class, read from the executable (table addresses).
const VM_TABLES: [(u32, u32); 16] = [
    (0x00, 0x801B_E674),
    (0x01, 0x801B_E7C4),
    (0x02, 0x801B_E89C),
    (0x03, 0x801B_EA8C),
    (0x04, 0x801B_EB5C),
    (0x05, 0x801B_EBE4),
    (0x06, 0x801B_E994),
    (0x08, 0x801B_E97C),
    (0x09, 0x801B_E9CC),
    (0x0A, 0x801B_E9EC),
    (0x0B, 0x801B_EA24),
    (0x0C, 0x801B_EA34),
    (0x0D, 0x801B_EA74),
    (0x0E, 0x801B_EBDC),
    (0x0F, 0x801B_EC5C),
    (0x10, 0x801B_EA5C),
];

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn i16_at(d: &[u8], o: usize) -> i64 {
    u16_at(d, o) as i16 as i64
}
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

/// Python's `b[a:b]` (clipped, empty when b <= a).
pub fn slice(d: &[u8], a: usize, b: usize) -> &[u8] {
    let b = b.min(d.len());
    if a >= b { &[] } else { &d[a..b] }
}

/// `bytes.hex()` / `bytes.hex(' ')`.
pub fn hex(d: &[u8], sep: bool) -> String {
    let mut s = String::with_capacity(d.len() * 3);
    for (i, b) in d.iter().enumerate() {
        if sep && i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// `'%0NX' % v` for a Python int (a negative one gets a minus sign inside the width).
pub fn phex(v: i64, width: usize) -> String {
    if v < 0 {
        let d = format!("{:X}", -v);
        format!("-{:0>w$}", d, w = width.saturating_sub(1))
    } else {
        format!("{:0>w$}", format!("{v:X}"), w = width)
    }
}

/// `'%-Ns' % s`.
fn ljust(s: &str, n: usize) -> String {
    format!("{s:<n$}")
}

pub type VmSizes = Vec<(u32, Vec<(u32, u32)>)>;

pub fn load_vm_tables(exe: &[u8]) -> VmSizes {
    let mut sizes = Vec::new();
    for (op, addr) in VM_TABLES {
        let mut lst = Vec::new();
        for s in 0..64 {
            let o = (addr - LOAD) as usize + 8 * s;
            if o + 8 > exe.len() {
                break;
            }
            let (f, sz) = (u32_at(exe, o), u32_at(exe, o + 4));
            if !(EXE_LO..EXE_HI).contains(&f) || !(1..=64).contains(&sz) {
                break;
            }
            lst.push((f, sz));
        }
        sizes.push((op, lst));
    }
    sizes
}

pub fn vm_tables_json(sizes: &VmSizes) -> Json {
    let mut o = Obj::new();
    for (op, lst) in sizes {
        let v: Vec<Json> =
            lst.iter().enumerate().map(|(s, (f, sz))| Json::Obj(Obj::new().with("sub", s).with("handler", format!("0x{f:08X}")).with("size", *sz))).collect();
        o.set(&format!("op{op:02X}"), v);
    }
    Json::Obj(o)
}

/// One effect script (container entry starting with u32 0x1C): (json, listing).
pub fn disassemble_effect(e: &[u8], sizes: &VmSizes) -> (Json, String) {
    let hdr: Vec<u32> = (0..7).map(|i| u32_at(e, 4 * i)).collect();
    let size = (hdr[2] as usize).min(e.len());
    let mut p = hdr[0] as usize;
    let mut insns = Vec::new();
    let mut targets = std::collections::HashSet::new();
    let table = |op: u32| sizes.iter().find(|x| x.0 == op).map(|x| &x.1);
    while p + 4 <= size {
        let w = u32_at(e, p);
        let (op, sub, fld) = (w & 0xFF, (w >> 8) & 0xFF, i16_at(e, p + 2));
        let (ln, name) = if op == 7 {
            if sub == 4 {
                targets.insert(fld);
            }
            let n = match sub {
                4 => "goto".to_string(),
                0x0B => "end".to_string(),
                0x0C => "destroy".to_string(),
                _ => format!("call_helper_{sub:02X}"),
            };
            (4usize, n)
        } else if let Some(t) = table(op).filter(|t| (sub as usize) < t.len()) {
            (t[sub as usize].1 as usize, format!("op{op:02X}.{sub:02X}"))
        } else {
            break;
        };
        if p + ln > size {
            break;
        }
        let args: Vec<i64> = if ln > 4 { (0..(ln - 4) / 4).map(|k| u32_at(e, p + 4 + 4 * k) as i32 as i64).collect() } else { Vec::new() };
        let handler = if op != 7 { Some(format!("0x{:08X}", table(op).unwrap()[sub as usize].0)) } else { None };
        insns.push((p, op, sub, fld, args, name, handler));
        p += ln;
    }
    let data = slice(e, p, size);
    let mut entries: Vec<u32> = [hdr[0], hdr[1], hdr[4]].into_iter().filter(|&x| x != 0).collect();
    entries.sort();
    entries.dedup();
    let mut out = vec![format!("; effect script  size={}  entries={}", size, entries.iter().map(|x| format!("0x{x:04X}")).collect::<Vec<_>>().join(", "))];
    for (off, op, sub, fld, args, name, handler) in &insns {
        let mark = if entries.contains(&(*off as u32)) { " <entry>" } else { "" };
        let lab = if targets.contains(&(*off as i64)) { " <target>" } else { "" };
        if *op == 7 {
            let arg = if *sub == 4 { format!("-> 0x{}", phex(*fld, 4)) } else { String::new() };
            out.push(format!("{}  {} {}{}{}", phex(*off as i64, 4), ljust(name, 14), arg, mark, lab));
        } else {
            let a = args.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ");
            out.push(format!(
                "{}  {} obj+0x{} {}   ; handler {}{}{}",
                phex(*off as i64, 4),
                ljust(name, 14),
                phex(*fld & 0xFFFF, 2),
                if a.is_empty() { String::new() } else { format!("[{a}]") },
                handler.as_deref().unwrap_or("None"),
                mark,
                lab
            ));
        }
    }
    if !data.is_empty() {
        out.push(format!("; data block at 0x{} ({} bytes)", phex(p as i64, 4), data.len()));
        for k in (0..data.len()).step_by(16) {
            out.push(format!("{}  {}", phex((p + k) as i64, 4), hex(slice(data, k, k + 16), true)));
        }
    }
    let ins_json: Vec<Json> = insns
        .iter()
        .map(|(off, op, sub, fld, args, name, handler)| {
            Json::Obj(
                Obj::new()
                    .with("offset", *off)
                    .with("op", *op)
                    .with("sub", *sub)
                    .with("field", *fld)
                    .with("args", args.clone())
                    .with("name", name.as_str())
                    .with("handler", handler.clone()),
            )
        })
        .collect();
    let js = Obj::new()
        .with("header", hdr.clone())
        .with("entries", entries.clone())
        .with("instructions", ins_json)
        .with("data_offset", p)
        .with("data_hex", hex(data, false))
        .with("complete", p == size || !data.is_empty());
    (Json::Obj(js), out.join("\n"))
}

/// 0xED1C4 entries: 32-bit fields, pointers into the EXE annotated.
pub fn decode_emitter(e: &[u8]) -> Json {
    let n = e.len() / 4;
    let words: Vec<i64> = (0..n).map(|i| u32_at(e, 4 * i) as i32 as i64).collect();
    let mut fields = Vec::new();
    for (i, &v) in words.iter().enumerate() {
        let mut f = Obj::new().with("offset", 4 * i).with("value", v);
        let vo = v | 0x8000_0000;
        if (EXE_LO as i64) <= vo && vo < EXE_HI as i64 && (0x10_0000..0x20_0000).contains(&v) {
            f.set("code_ptr", format!("0x{vo:08X}"));
        } else if v == 4096 {
            f.set("note", "1.0 (fixed 4096)");
        }
        fields.push(Json::Obj(f));
    }
    Json::Obj(Obj::new().with("magic", format!("0x{:X}", words[0])).with("size", words[1]).with("fields", fields))
}

fn s10(v: u32) -> i64 {
    crate::model::s10(v)
}

// --- move scripts (.ANI): interpreter 0x8017E824, handler table 0x801B4C10[byte 0] (48 ops) ------------------------
const MS_SIZE: [usize; 48] =
    [2, 2, 2, 2, 4, 4, 2, 6, 4, 6, 6, 2, 4, 4, 4, 4, 4, 4, 2, 2, 4, 6, 2, 4, 6, 4, 4, 2, 2, 6, 4, 4, 4, 6, 4, 4, 2, 8, 4, 6, 4, 4, 2, 6, 2, 2, 2, 4];

fn ms_name(op: u8) -> Option<&'static str> {
    Some(match op {
        0x06 => "flip_side",
        0x0B => "nop",
        0x16 => "spawn_object",
        0x18 => "effect",
        0x2A => "halt",
        0x21 | 0x22 | 0x2C => "test",
        0x2F => "count_test",
        0x23 => "set_bytes_101",
        _ => return None,
    })
}

fn ms_hint(op: u8) -> Option<&'static str> {
    Some(match op {
        0x07 => "calls 0x8018D040",
        0x08 => "writes +0xD0..0xE4",
        0x09 => "writes fighter +0x8A..0x98",
        0x0C => "anim timing +0xC2..0xCC, calls 0x80185B84",
        0x0D => "calls 0x8013F6D4/0x80140188/0x80156EAC",
        0x0E => "writes +0xDA/+0xDC/+0x3A",
        0x0F => "writes +0xDB/+0xC6",
        0x10 => "writes +0x101..0x104, +0xB8..0xBC",
        0x11 => "writes +0x10A/+0x10C",
        0x12 => "calls 0x80175BB0",
        0x13 => "writes fighter +0x27",
        0x14 => "writes fighter +0xC0..0xD4",
        0x17 => "many calls (0x8013xxxx)",
        0x19 => "calls 0x80140C60/0x80142CE0",
        0x1A => "calls 0x80179888",
        0x1C => "calls 0x80185A18",
        0x1D => "calls 0x8017A5C0",
        0x1F => "writes +0x98/+0xB2/+0xB4/+0xC4",
        0x20 => "writes +0x20..0x2A",
        0x24 => "input test, calls 0x801356E4",
        0x25 => "writes fighter +0x14",
        0x26 => "calls rand 0x80129F38",
        0x27 => "writes +0x10E/+0x110",
        0x28 => "writes +0x10A/+0x10C",
        0x2B => "reads 3 halfwords",
        0x2D => "calls 0x80140C60/0x80175BB0",
        0x2E => "writes global block at *0x1F8000BC",
        _ => return None,
    })
}

const MS_FLAGW: [&str; 5] = ["F58", "F60", "F5C", "F64", "F68"];

fn ms_cond(c: u8) -> String {
    match c {
        1 => "if !res".into(),
        2 => "if res".into(),
        3 => "if res>=0".into(),
        4 => "if res<0".into(),
        10 => "if !beast".into(),
        11 => "if beast".into(),
        12 => "if timer==0 && +0xCA<=0".into(),
        13 => "if +0 & 0x80".into(),
        14 => "if +5 & 0x10 || +0xD6>=0x1E00".into(),
        16 => "if state==0".into(),
        _ => format!("if state=={}", c as i64 - 16),
    }
}

fn ms_call(c: u8) -> Option<&'static str> {
    Some(match c {
        5 => "call",
        6 => "if !res call",
        7 => "if res call",
        8 => "if res>=0 call",
        9 => "if res<0 call",
        _ => return None,
    })
}

fn ms_ctl(s8: i64) -> (&'static str, bool) {
    match s8 {
        0 => ("end", true),
        1 => ("nop", false),
        2 => ("ret", true),
        3 => ("ret if !res", false),
        4 => ("ret if res", false),
        5 => ("ret if res>=0", false),
        6 => ("ret if res<0", false),
        7 => ("yield", false),
        8 => ("native 0x8012B42C", false),
        _ => ("ret", true),
    }
}

fn ms_size(b: &[u8], p: usize) -> Option<usize> {
    let (op, sub) = (b[p], b[p + 1]);
    if op >= 0x30 {
        return None;
    }
    if op == 0x23 {
        return Some(if sub & 0xF == 4 { 6 } else { 4 });
    }
    if op == 0x2E {
        let s = sub as i8 as i64;
        return Some(if [0, 5, 6, 7, 8, 9].contains(&s) {
            4
        } else if [1, 2, 3, 4].contains(&s) {
            8
        } else {
            2
        });
    }
    Some(MS_SIZE[op as usize])
}

/// (text, size, jump target, call target, falls through)
fn ms_decode(b: &[u8], p: usize) -> (String, usize, Option<usize>, Option<usize>, bool) {
    let (op, sub) = (b[p], b[p + 1]);
    let n = ms_size(b, p).unwrap();
    let hw: Vec<u16> = (0..n / 2).map(|i| u16_at(b, p + 2 * i)).collect();
    let s8 = sub as i8 as i64;
    let who = format!(
        "{}{}",
        if sub & 0x40 != 0 { "opp." } else { "" },
        match sub & 0x30 {
            0x10 => "[human] ",
            0x20 => "[beast] ",
            _ => "",
        }
    );
    match op {
        0 => {
            let (name, stop) = ms_ctl(s8);
            (name.into(), n, None, None, !stop)
        }
        1 => {
            let t = match sub {
                0 => "loop".to_string(),
                1 => "loop_start".to_string(),
                _ => format!("loop?{sub}"),
            };
            (t, n, None, None, sub != 0)
        }
        2 => (if s8 >= 0 { format!("wait_frame {s8}") } else { format!("wait_until cond{}", s8 + 7) }, n, None, None, true),
        3 => (format!("sleep {}", if sub == 0xFF { "default".to_string() } else { (sub as u32 * 16).to_string() }), n, None, None, true),
        4 => {
            let (c, gate, t) = (sub & 0x3F, sub >> 6, hw[1] as usize);
            let g = match gate {
                1 => "[human] ",
                2 => "[beast] ",
                3 => "opp.",
                _ => "",
            };
            let g12 = if gate == 1 || gate == 2 { g } else { "" };
            if c == 0 || c == 15 {
                return (format!("{}goto L_{:04X}", if gate == 0 || gate == 3 { "" } else { g }, t), n, Some(t), None, gate == 1 || gate == 2);
            }
            if let Some(call) = ms_call(c) {
                return (format!("{g12}{call} S_{t:04X}"), n, None, Some(t), true);
            }
            let mut cond = ms_cond(c);
            if gate == 3 {
                cond = cond.replace("if ", "if opp.");
            }
            (format!("{g12}{cond} goto L_{t:04X}"), n, Some(t), None, true)
        }
        5 => {
            const NAMES: [&str; 8] = ["anim", "start_move", "set +0x98", "set +0x9A", "set +0x16", "opp_anim", "set +0x14", "set slot"];
            (format!("{who}{} {}", NAMES[(sub & 7) as usize], i16_at(b, p + 2)), n, None, None, true)
        }
        0xA => {
            let v = hw[1] as u32 | (hw[2] as u32) << 16;
            let s = sub & 0x3F;
            let (w, k) = if s < 0x19 {
                (MS_FLAGW[(s % 5) as usize], (s / 5) as usize)
            } else if s < 0x1E {
                ("F6C", (s - 0x19) as usize)
            } else {
                ("F54", (s - 0x1E) as usize)
            };
            let ex = match k {
                0 => format!("{w} = 0x{v:08X}"),
                1 => format!("{w} &= 0x{v:08X}"),
                2 => format!("{w} |= 0x{v:08X}"),
                3 => format!("{w} ^= 0x{v:08X}"),
                _ => format!("res = {w} & 0x{v:08X}"),
            };
            (format!("{}{ex}", if sub & 0xC0 == 0xC0 { "opp." } else { "" }), n, None, None, true)
        }
        0x29 => (format!("if rand >= {sub} goto L_{:04X}", hw[1]), n, Some(hw[1] as usize), None, true),
        0x2A => ("halt".into(), n, None, None, false),
        _ => {
            let name = ms_name(op).map(String::from).unwrap_or_else(|| format!("op{op:02X}"));
            let args = hw[1..].iter().map(|x| format!("{x:04X}")).collect::<Vec<_>>().join(" ");
            let t = format!("{name}.{sub:02X} {args}").trim_end().to_string();
            (t + &ms_hint(op).map(|h| format!("   ; {h}")).unwrap_or_default(), n, None, None, true)
        }
    }
}

/// Recursive descent over every script entry and branch / call target: (listing, stats).
pub fn disassemble_moves(ani: &[u8], entries: &[u16], end: usize) -> (String, Json) {
    let mut roots: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    let mut root_order = Vec::new();
    for (i, &e) in entries.iter().enumerate() {
        let e = e as usize;
        if 0 < e && e < end {
            if !roots.contains_key(&e) {
                root_order.push(e);
            }
            roots.entry(e).or_default().push(i);
        }
    }
    let mut insn: std::collections::BTreeMap<usize, (String, usize)> = Default::default();
    let (mut labels, mut subs) = (std::collections::HashSet::new(), std::collections::HashSet::new());
    let mut todo = root_order.clone();
    let lo = roots.keys().next().copied().unwrap_or(0) as i64;
    while let Some(mut p) = todo.pop() {
        while lo - 0x400 <= p as i64 && p < end && !insn.contains_key(&p) {
            if ms_size(ani, p).is_none() {
                break;
            }
            let (text, n, jt, ct, cont) = ms_decode(ani, p);
            insn.insert(p, (text, n));
            if let Some(t) = jt {
                labels.insert(t);
                todo.push(t);
            }
            if let Some(t) = ct {
                subs.insert(t);
                todo.push(t);
            }
            if !cont {
                break;
            }
            p += n;
        }
    }
    let mut lines = Vec::new();
    let mut last: Option<usize> = None;
    for (&p, (text, n)) in &insn {
        if let Some(l) = last
            && p > l
        {
            lines.push(format!("        db {}", hex(slice(ani, l, p), true)));
        }
        if let Some(r) = roots.get(&p) {
            lines.push(String::new());
            lines.push(format!("move_{}:", r.iter().map(|i| format!("{i:03}")).collect::<Vec<_>>().join(",")));
        } else if subs.contains(&p) {
            lines.push(String::new());
            lines.push(format!("S_{p:04X}:"));
        }
        if labels.contains(&p) {
            lines.push(format!("L_{p:04X}:"));
        }
        lines.push(format!("  {p:04X}  {} {}", ljust(&hex(slice(ani, p, p + n), true), 17), text));
        last = Some(p + n);
    }
    let covered: usize = insn.values().map(|x| x.1).sum();
    let nsubs = subs.iter().filter(|s| !roots.contains_key(s)).count();
    let overlaps: usize = insn.iter().map(|(&q, (_, n))| (q + 1..q + n).filter(|r| insn.contains_key(r)).count()).sum();
    let stats = Obj::new()
        .with("instructions", insn.len())
        .with("entries", roots.len())
        .with("subroutines", nsubs)
        .with("bytes_decoded", covered)
        .with("overlaps", overlaps);
    let mut head = vec![
        format!("; move scripts  entries={}  instructions={}  subroutines={}  overlaps={}", roots.len(), insn.len(), nsubs, overlaps),
        "; res = test result (+0x94); F54..F6C = state flag words; S_xxxx = subroutine (return with \"ret\")".to_string(),
    ];
    head.extend(lines);
    (head.join("\n") + "\n", Json::Obj(stats))
}

fn words10(ani: &[u8], addr: usize, cnt: usize) -> Vec<Json> {
    (0..cnt)
        .map(|i| {
            let w = u32_at(ani, addr + 4 * i);
            Json::from(vec![s10(w), s10(w >> 10), s10(w >> 20)])
        })
        .collect()
}

/// The move-script entry table of an ANI: (entries, end of the script blob).
pub fn move_entries(ani: &[u8]) -> (Vec<u16>, usize) {
    let h: Vec<usize> = (0..9).map(|i| u32_at(ani, 4 * i) as usize).collect();
    ((0..(h[3] - h[1]) / 2).map(|i| u16_at(ani, h[1] + 2 * i)).collect(), h[6])
}

/// Every section of a `<CODE><n>P.ANI` (`gamedata.decode_ani`).
pub fn decode_ani(ani: &[u8]) -> Json {
    let h: Vec<usize> = (0..9).map(|i| u32_at(ani, 4 * i) as usize).collect();
    let (total, s_entries, s_groups, s_moves, s_tab4, s_tab5, s_skel, s_maps, s_params) = (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], h[8]);
    let mut out = Obj::new().with(
        "header",
        Obj::new()
            .with("size", total)
            .with("script_entry_table", s_entries)
            .with("anim_group_table", s_groups)
            .with("move_records", s_moves)
            .with("offset_table_4", s_tab4)
            .with("offset_table_5", s_tab5)
            .with("skeleton_descriptors", s_skel)
            .with("bone_maps", s_maps)
            .with("params", s_params),
    );
    out.set("params", (0..12).map(|i| u16_at(ani, 0x24 + 2 * i)).collect::<Vec<_>>());
    let mut caps = Vec::new();
    for k in 0..4 {
        let (cnt, off) = (u16_at(ani, s_params + 2 * k) as usize, u32_at(ani, s_params + 8 + 4 * k) as usize);
        let c: Vec<Json> = (0..cnt)
            .map(|i| {
                let o = off + 4 * i;
                Json::Obj(Obj::new().with("node_a", ani[o]).with("node_b", ani[o + 1]).with("radius", u16_at(ani, o + 2)))
            })
            .collect();
        caps.push(Json::Obj(Obj::new().with("offset", off).with("capsules", c)));
    }
    out.set(
        "collision_capsules",
        Obj::new().with("tables", caps).with("note", "4 tables (2 per form: human / beast); each capsule joins two skeleton nodes with a radius"),
    );
    let mut maps = Obj::new();
    for k in 0..4 {
        let o = u32_at(ani, s_maps + 4 * k) as usize;
        maps.set(&format!("form{}_{}", k / 2, if k % 2 == 0 { "normal" } else { "mirrored" }), slice(ani, o, o + 20).to_vec());
    }
    out.set("bone_maps", maps);
    let first = u32_at(ani, s_groups) as usize;
    let mut groups = Vec::new();
    let mut g = s_groups;
    while g < first && g + 4 <= ani.len() {
        let o = u32_at(ani, g) as usize;
        groups.push(Json::from((0..16).map(|i| u16_at(ani, o + 2 * i)).collect::<Vec<_>>()));
        g += 4;
    }
    let ngroups = groups.len();
    out.set("anim_groups", groups);
    let mut skel = Vec::new();
    for form in 0..2 {
        let d: Vec<usize> = (0..7).map(|i| u32_at(ani, s_skel + 28 * form + 4 * i) as usize).collect();
        let parent: Vec<i64> = (0..32).map(|i| i16_at(ani, d[0] + 2 * i)).collect();
        let n = (2..32).find(|&i| parent[i] == 0 && i > 18).unwrap_or(32);
        let node_map: Vec<i64> = (0..24).map(|i| i16_at(ani, d[1] + 2 * i)).collect();
        let mut extras = Vec::new();
        let mut a = d[5];
        for node in 19..32 {
            let (hdr, cnt) = (u16_at(ani, a), i16_at(ani, a + 2));
            if !(0..=32).contains(&cnt) {
                break;
            }
            extras.push(Json::Obj(
                Obj::new()
                    .with("node", node)
                    .with("type", hdr & 0xFF)
                    .with("chain_start", hdr & 0x8000 != 0)
                    .with("chain_end", hdr & 0x4000 != 0)
                    .with("params", (0..cnt as usize).map(|i| i16_at(ani, a + 4 + 2 * i)).collect::<Vec<_>>()),
            ));
            a += 4 + 2 * cnt as usize;
        }
        let base: Vec<Json> = (0..n - 1)
            .map(|i| {
                let w = u32_at(ani, d[4] + 4 * i);
                Json::from(vec![s10(w) * 4, s10(w >> 10) * 4, s10(w >> 20) * 4])
            })
            .collect();
        skel.push(Json::Obj(
            Obj::new()
                .with("parent", parent[..n].to_vec())
                .with("coord_to_node", node_map)
                .with("bone_offsets", words10(ani, d[2], n - 1))
                .with("unknown_3", words10(ani, d[3], n - 1))
                .with("base_rotation_euler", base)
                .with("extra_bones", extras)
                .with("unknown_6", (0..24).map(|i| i16_at(ani, d[6] + 2 * i)).collect::<Vec<_>>()),
        ));
    }
    out.set("skeleton", skel);
    let blob_start = first + 32 * ngroups;
    let (ent, _) = move_entries(ani);
    let (_, stats) = disassemble_moves(ani, &ent, s_skel);
    out.set(
        "move_scripts",
        Obj::new()
            .with("blob_offset", blob_start)
            .with("blob_size", s_skel as i64 - blob_start as i64)
            .with("entry_offsets", ent.clone())
            .with("blob_hex", hex(slice(ani, blob_start, s_skel), false))
            .with("disassembly", stats)
            .with("note", "offsets are relative to the ANI start; listing in data/ani/<name>_moves.txt"),
    );
    let mut recs: Vec<Vec<i64>> = Vec::new();
    let mut k = s_moves;
    while k + 16 <= s_tab4 {
        let r: Vec<i64> = (0..6).map(|i| u16_at(ani, k + 2 * i) as i64).chain([u32_at(ani, k + 12) as i64]).collect();
        if r[6] != 0 && (r[6] as usize) < k + 16 {
            break;
        }
        recs.push(r);
        k += 16;
        let m = recs.iter().filter(|x| x[6] != 0).map(|x| x[6] as usize).min().unwrap_or(s_tab4);
        if k >= m {
            break;
        }
    }
    out.set(
        "move_records",
        Obj::new()
            .with("records", recs.into_iter().map(|r| Json::Obj(Obj::new().with("raw", r))).collect::<Vec<_>>())
            .with("tail_hex", hex(slice(ani, k, s_tab4), false)),
    );
    for (name, start, end) in [("offset_table_4", s_tab4, s_tab5), ("offset_table_5", s_tab5, total)] {
        let (mut offs, mut z) = (Vec::new(), start);
        while z + 4 <= end {
            let v = u32_at(ani, z) as usize;
            if !(start < v && v <= total) {
                break;
            }
            offs.push(v);
            z += 4;
        }
        out.set(name, Obj::new().with("offsets", offs).with("data_hex", hex(slice(ani, z, end), false)));
    }
    Json::Obj(out)
}

/// The runtime's static tables (`engine/tables.okt`, `okt`; documented by name in `docs/tables.md`): name, element
/// type, count and the RAM address of the first byte (the table's `origin`). Each is the executable's bytes there,
/// verbatim; the linked ones (`fx.*`, `fxvm.*`) hold addresses of their own records, which keep pointing into their
/// origins. Ordered by address and never overlapping (`okt_tables_cover_the_recorded_reads` checks both, and that every
/// byte of the recorded reads, `tests/fixtures/runtime_reads.txt`, is in one).
pub const TABLES: &[(&str, Elem, u32, u32)] = &[
    // the random byte table of the CPU and the effects
    ("math.random_bytes", Elem::U8, 0x100, 0x8011_EC40),
    // the fonts' metrics: the advance widths of fonts 0..4 and font 3's glyph cells (offsets in docs/tables.md)
    ("font.metrics", Elem::Blob, 0x224, 0x8011_ED80),
    // per stage ST0..STC: the fighters' light (0x44-byte records)
    ("stage.light", Elem::Rec(0x44), 13, 0x8011_F484),
    // the TV camera's eye positions (8-byte vectors), its random eye positions (0x801306D4), its height per mode, the
    // throw camera's angles (angle, flag) and the tracking shot's yaw offsets
    ("camera.presets", Elem::Rec(8), 8, 0x8011_F944),
    ("camera.random", Elem::Rec(8), 8, 0x8011_F9C4),
    ("camera.heights", Elem::I16, 4, 0x8011_FA04),
    ("camera.throw", Elem::Rec(4), 8, 0x8011_FD78),
    ("camera.track", Elem::I16, 4, 0x8011_FD98),
    // per stage: the walls (model, hit points), the backdrop
    ("stage.walls", Elem::Rec(16), 13, 0x8011_FDA0),
    ("stage.backdrop", Elem::Rec(26), 13, 0x8011_FEA0),
    // trigonometry: arctangent, tangent (0x3AF entries), sine (a quarter turn, 0x401 entries)
    ("math.atan", Elem::I16, 0x401, 0x8012_3840),
    ("math.tan", Elem::I16, 0x3AF, 0x8012_4044),
    ("math.sin", Elem::I16, 0x401, 0x8012_47A4),
    // the screen titles' banner widths, entry for entry with the text list `screen_title` (0x8012BEE4)
    ("screen.title_widths", Elem::U32, 24, 0x801B_10F0),
    // per stage: the ring the drive grid is laid over, the ring, the outer ring (kind, drop, half x, half z), the
    // drive grid's height
    ("stage.grid_ring", Elem::Rec(8), 13, 0x801B_12CC),
    ("stage.ring", Elem::Rec(8), 13, 0x801B_1334),
    ("stage.ring_outer", Elem::Rec(8), 13, 0x801B_139C),
    ("stage.drive_grid", Elem::U32, 13, 0x801B_141C),
    // per stage ST0..ST9: the shadows' floor (8-byte records: shape, height off the ring, half x, half z)
    ("stage.shadow_floor", Elem::Rec(8), 10, 0x801B_145C),
    // the hit sparks' sprites: sizes, CLUTs, the stage selector, texture pages, UVs, the keyframe tracks and their
    // pointer table, the per-sprite colours
    ("fx.hit_sprites", Elem::Blob, 0x414, 0x801B_1650),
    // the effect script VM's static block (the UV animator's deltas and script), mapped at its origin
    ("fxvm.static_block", Elem::Blob, 0x2C, 0x801B_1A64),
    // the emitters' timelines: the records, the four the effect op picks from, two the move scripts start
    ("fxvm.timeline_records", Elem::Blob, 0x50, 0x801B_1A90),
    ("fxvm.timelines", Elem::U32, 4, 0x801B_1AE0),
    ("fxvm.emitter_timelines", Elem::Blob, 0x28, 0x801B_1AF0),
    // the lens flare of City and Lab: the models, their depths, the positions (8-byte vectors, two rows of five)
    ("fx.lens_flare", Elem::Blob, 0x64, 0x801B_1B18),
    // the colour flash patterns (a count, then that many RGB triples of i16) and their pointer table (linked)
    ("fx.colour_flash", Elem::Blob, 0xD0, 0x801B_1B7C),
    // the moves' effects: claw and kick trails, rays, streaks, the sparkle schedules (with their pointer table), the
    // rays' speeds and widths, the streaks' CLUTs; ends at 0x801B20D8 (0x801B20FC.. is the front end's mode menu)
    ("fx.move_effects", Elem::Blob, 0x46C, 0x801B_1C6C),
    // the mode menu's half widths (centring its 7 items: ARCADE VS STORY SURVIVAL WATCH CUSTOM OPTIONS)
    ("menu.mode_half", Elem::U8, 7, 0x801B_20FC),
    // the NORMAL select's face order
    ("select.normal_order", Elem::U8, 11, 0x801B_2136),
    // the VS screen's announcer pauses after the name and after the beast word, per character
    ("vs.announcer_delays", Elem::Rec(2), 11, 0x801B_2164),
    // the HUD frames' pieces: texture rectangles (u, v, w, h) of the corners TL TR BL BR, the top and bottom edges, the
    // right and left sides
    ("hud.frame", Elem::Rec(4), 8, 0x801B_2F38),
    // the character select: the cursor's neighbours (up, right, down, left), the face cells, the name plates' x, the
    // STORY select's beast picture x, the stage names' half widths, its title x and name line x
    ("select.neighbours", Elem::Rec(4), 11, 0x801B_2FC0),
    ("select.faces", Elem::Rec(4), 11, 0x801B_2FEC),
    ("select.plates", Elem::Rec(2), 11, 0x801B_3018),
    ("select.story_beast_x", Elem::U8, 11, 0x801B_3030),
    ("select.stage_half", Elem::U8, 9, 0x801B_3064),
    ("select.story_title_x", Elem::U8, 11, 0x801B_309C),
    ("select.story_name_x", Elem::U8, 11, 0x801B_30A8),
    // the front end's sound definitions of the ending bank (6-byte records, the volume at +4)
    ("sound.menu_volumes", Elem::Rec(6), 6, 0x801B_3500),
    // the movies' records (28 bytes, the last frame at +8), the default button configuration, the arcade rank table
    // (DIFFICULTY × fight: 16-byte rows), the controller config's values
    ("movie.records", Elem::Rec(28), 4, 0x801B_3C80),
    ("input.default_buttons", Elem::U16, 8, 0x801B_3D20),
    ("progress.rank", Elem::Rec(16), 8, 0x801B_3D30),
    ("menu.cc_values", Elem::U16, 10, 0x801B_3F7C),
    // STORY mode: the fights per character, their stages and opponents (8 a character), the page script (25 bytes)
    ("story.fight_count", Elem::U8, 11, 0x801B_43C4),
    ("story.stages", Elem::Rec(8), 11, 0x801B_43D0),
    ("story.opponents", Elem::Rec(8), 11, 0x801B_4428),
    ("story.script", Elem::Rec(25), 11, 0x801B_4480),
    // per stage: the object slots the stage draw fills
    ("stage.object_slots", Elem::Rec(23), 13, 0x801B_4620),
    // the HUD: the names' x, the sprites' texture rectangles (u, v, w, h) and their animations
    ("hud.data", Elem::Blob, 0x3B8, 0x801B_47D8),
    // the BEAST pop-up's palette cycle ((row, frames) pairs up to 0xFF00, a pad) and its 4 CLUT ids (0x80173AB0)
    ("hud.popup", Elem::U16, 20, 0x801B_4B90),
    // the CPU's threshold tables
    ("ai.cpu", Elem::U8, 0xA0, 0x801B_4FD0),
    // the CPU's recovery pads
    ("ai.recover_pads", Elem::U16, 8, 0x801B_5118),
    // square roots and reciprocal square roots (192 entries each)
    ("math.sqrt", Elem::I16, 192, 0x801B_8228),
    ("math.rsqrt", Elem::I16, 192, 0x801B_83BC),
];

/// The text the game draws from the game's own lists, by the game's pointer tables: key prefix, the table's address,
/// its entries (`<prefix>.<n>` for entry n; `names`: each character's name, then its beast type), whether the trailing
/// spaces are cut (`c_string`; not for `censor`, whose words the game compares as 3 bytes, spaces included).
pub const TEXT_LISTS: &[(&str, u32, u32, bool)] = &[
    ("names", 0x801B_4594, 22, true),
    ("story_title", 0x801B_3070, 11, true),
    ("modes", 0x801B_2104, 7, true),
    ("screen_title", 0x801B_1150, 24, true),
    ("stages", 0x801B_303C, 10, true),
    ("displays", 0x801B_3F10, 5, true),
    ("cc_names", 0x801B_3F54, 10, true),
    ("cc_icons", 0x801B_3F24, 8, true),
    ("movies", 0x801B_3ED4, 4, true),
    ("censor", 0x801B_3FA4, 22, false),
    ("ordinal", 0x801B_3FFC, 10, true),
    ("se_banks", 0x801B_3E48, 4, true),
];

/// The other text the game draws, by key: the C string at the address, its trailing spaces cut (`text_table`).
pub const TEXT: &[(&str, u32)] = &[
    ("1_block_is_required_to_save", 0x8012_1824),
    ("afterimage_mode", 0x8012_12F4),
    ("all_members", 0x8012_122C),
    ("any_cancel_point", 0x8012_12A8),
    ("attack_level", 0x8012_138C),
    ("auto_save", 0x8012_1B6C),
    ("awaiting_challenger", 0x8012_1EA4),
    ("back", 0x8012_2C1C),
    ("beast", 0x8012_06B4),
    ("bgm_player", 0x8012_15E8),
    ("bgm_select", 0x8012_15C0),
    ("bgm_volume", 0x8012_15DC),
    ("big_arms", 0x8012_123C),
    ("big_head", 0x8012_1250),
    ("blocks_after_taking_damage", 0x8012_1DA4),
    ("blocks_and_1_block_is_required", 0x8012_19B8),
    ("blood_effect", 0x8012_1298),
    ("challenger", 0x8012_0900),
    ("champion_1_win", 0x8012_089C),
    ("champion_n_wins", 0x8012_08AC),
    ("character_select", 0x8012_06A0),
    ("charge_speed", 0x8012_12BC),
    ("checking_memory_card_n", 0x8012_1938),
    ("clear", 0x8012_06BC),
    ("com_action", 0x8012_1DDC),
    ("com_defensive_fall", 0x8012_1E0C),
    ("com_guard", 0x8012_1DE8),
    ("com_guard_and_escape", 0x8012_1DF4),
    ("completed", 0x8012_1954),
    ("config", 0x8012_1D80),
    ("console", 0x8012_1434),
    ("continue", 0x8012_22A4),
    ("continue_n", 0x8012_0824),
    ("controller", 0x8012_1D74),
    ("counts_rate_win_rate_win_rate", 0x8012_1C24),
    ("crouching", 0x8012_1D98),
    ("custom_mode", 0x8012_1E8C),
    ("d_k_o", 0x8012_07EC),
    ("data_error", 0x8012_1A24),
    ("default", 0x8012_1310),
    ("demonstration", 0x8012_20E0),
    ("difficulty", 0x8012_1380),
    ("display", 0x8012_1C80),
    ("display_type", 0x8012_13D0),
    ("do_not_remove_memory_card", 0x8012_18D0),
    ("dodging", 0x8012_1D88),
    ("draw", 0x8012_07F8),
    ("error", 0x8012_1A34),
    ("exit", 0x8011_F110),
    ("fight", 0x8012_07B8),
    ("fight_n", 0x8012_072C),
    ("final_fight", 0x8012_0714),
    ("final_round", 0x8012_0794),
    ("final_stage", 0x8012_06FC),
    ("format_memory_card_n_ask", 0x8012_17D8),
    ("format_memory_card_n_done", 0x8012_1920),
    ("game_over", 0x8012_0834),
    ("game_start", 0x8012_1280),
    ("get_ready", 0x8012_07AC),
    ("hud", 0x801B_4024),
    ("jumping", 0x8012_1D90),
    ("k_o", 0x8012_07E4),
    ("kids", 0x8012_1248),
    ("letters", 0x801B_4164),
    ("load_from_memory_card_n_ask", 0x8012_1788),
    ("load_from_memory_card_n_done", 0x8012_18EC),
    ("load_from_memory_card_slot_1", 0x8012_1B78),
    ("master_volume", 0x8012_15CC),
    ("match_point", 0x8012_13B0),
    ("memory_card_changed", 0x8012_17A4),
    ("memory_card_n", 0x8012_1980),
    ("model_type", 0x8012_128C),
    ("mono", 0x8012_1424),
    ("movies", 0x8012_1750),
    ("no", 0x8012_1820),
    ("no_bloody_roar_2_data_detected", 0x8012_19E4),
    ("no_memory_card_is_inserted_in", 0x8012_1960),
    ("no_n", 0x8012_15F4),
    ("normal", 0x8012_125C),
    ("now_checking_memory_card_n", 0x8012_18B0),
    ("now_formatting_memory_card_n", 0x8012_188C),
    ("now_loading_from_memory_card", 0x8012_1844),
    ("now_saving_to_memory_card_n", 0x8012_186C),
    ("off", 0x8011_EC30),
    ("ok", 0x8011_F10C),
    ("on", 0x8011_EC38),
    ("out_of_ring", 0x8012_07CC),
    ("output_select", 0x8012_15B0),
    ("over_write_ok", 0x8012_180C),
    ("pause", 0x8012_1D50),
    ("perfect", 0x8012_07D8),
    ("pictures", 0x8012_1758),
    ("play_play_vs_com_vs_hum", 0x8012_1BFC),
    ("player_n", 0x8012_1D1C),
    ("please_wait", 0x8012_098C),
    ("practice_menu", 0x8012_1DCC),
    ("practice_mode", 0x8012_1E7C),
    ("press_start", 0x8012_1EB8),
    ("press_start_or_x_button", 0x8012_1E3C),
    ("quick", 0x8012_1338),
    ("quit", 0x8012_1D68),
    ("ranking", 0x8012_2048),
    ("recover_speed", 0x8012_12D4),
    ("replay", 0x8012_0800),
    ("resume", 0x8012_1D58),
    ("round", 0x8012_07A0),
    ("s_story", 0x8012_06C4),
    ("save_to_memory_card_n_ask", 0x8012_17BC),
    ("save_to_memory_card_n_done", 0x8012_1908),
    ("save_to_memory_card_slot_1", 0x8012_1B98),
    ("saving_to_memory_card_n", 0x8012_17F0),
    ("se_player", 0x8012_1604),
    ("se_type", 0x8012_15FC),
    ("select", 0x8012_0690),
    ("side_step", 0x8012_1304),
    ("slot_1", 0x8012_1780),
    ("small", 0x8012_2BC0),
    ("special_stage", 0x8012_06EC),
    ("stage", 0x8012_0688),
    ("stage_n", 0x8012_0708),
    ("standing", 0x8012_1DC0),
    ("stereo", 0x8012_141C),
    ("story_mode", 0x8012_1E98),
    ("sudden_death", 0x8012_0784),
    ("survival_mode", 0x8012_1E6C),
    ("there_is_insufficient_memory_card", 0x8012_1994),
    ("time_limit", 0x8012_139C),
    ("time_over", 0x8012_07C0),
    ("to_save", 0x8012_19D8),
    ("total", 0x8012_1C68),
    ("unformatted_memory_card", 0x8012_1A08),
    ("v_s_mode", 0x8012_1E54),
    ("vibration", 0x8012_1410),
    ("vs_computer", 0x8012_1214),
    ("vs_human", 0x8012_1220),
    ("vs_ranking", 0x8012_2068),
    ("wall_display", 0x8012_12E4),
    ("watch_mode", 0x8012_1E60),
    ("wide_1", 0x8012_1350),
    ("wide_2", 0x8012_1348),
    ("wide_3", 0x8012_1340),
    ("winner", 0x8012_0808),
    ("wins", 0x8012_1FE4),
    ("yes", 0x8012_181C),
    ("you_lose", 0x8012_0818),
    ("you_win", 0x8012_0810),
];

/// The C string at `addr` of `exe` (at most 256 bytes), with `trim` its trailing spaces and line ends cut (a reader
/// places and spaces the text itself); Err when it is not printable ASCII or not inside the executable.
fn c_string(exe: &[u8], addr: u32, trim: bool) -> Result<String, String> {
    let o = addr.checked_sub(LOAD).map(|o| o as usize).filter(|&o| o < exe.len()).ok_or_else(|| format!("text at {addr:08X}: outside the EXE"))?;
    let b = &exe[o..exe.len().min(o + 256)];
    let n = b.iter().position(|&c| c == 0).ok_or_else(|| format!("text at {addr:08X}: no end"))?;
    let s = std::str::from_utf8(&b[..n]).ok().filter(|s| s.bytes().all(|c| (0x20..0x7F).contains(&c) || c == b'\n'));
    s.map(|s| if trim { s.trim_end_matches([' ', '\n']) } else { s }.to_string()).ok_or_else(|| format!("text at {addr:08X}: not text"))
}

/// The table `text.strings` (`TEXT_LISTS`, then `TEXT`): each key, NUL, its text, NUL.
pub fn text_table(exe: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut put = |key: &str, text: String| {
        out.extend(key.as_bytes());
        out.push(0);
        out.extend(text.as_bytes());
        out.push(0);
    };
    for &(prefix, table, n, trim) in TEXT_LISTS {
        for k in 0..n {
            let o = (table + 4 * k).checked_sub(LOAD).map(|o| o as usize).ok_or("a text list outside the EXE")?;
            let p = exe.get(o..o + 4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).ok_or("a text list outside the EXE")?;
            put(&format!("{prefix}.{k}"), c_string(exe, p, trim)?);
        }
    }
    for &(key, addr) in TEXT {
        put(key, c_string(exe, addr, true)?);
    }
    Ok(out)
}

/// `engine/tables.okt` from the executable `exe` (`TABLES`); Err when the executable is too short.
pub fn tables_okt(exe: &[u8]) -> Result<Vec<u8>, String> {
    let mut tables: Vec<okt::Table> = TABLES
        .iter()
        .map(|&(name, elem, count, origin)| {
            let o = origin.wrapping_sub(LOAD) as usize;
            let n = count as usize * elem.size();
            let bytes = o.checked_add(n).and_then(|e| exe.get(o..e)).ok_or_else(|| format!("the EXE is too short for the table {name}"))?.to_vec();
            Ok(okt::Table { name: name.into(), elem, count, origin, bytes })
        })
        .collect::<Result<_, String>>()?;
    let text = text_table(exe)?;
    tables.push(okt::Table { name: "text.strings".into(), elem: Elem::Blob, count: text.len() as u32, origin: 0, bytes: text });
    okt::write(&tables)
}

/// The index entries cut from the executable: the stages' rings, the sound definitions …
/// (its static tables are `tables_okt`'s).
pub fn runtime_index(exe: &[u8], index: &mut Obj) {
    let o = |a: u32| (a - LOAD) as usize;
    // ring of every stage (0x801B1334, 8 bytes per stage ST0..STC): kind, wall strength, half size x / z
    let rings: Vec<Json> = (0..13).map(|i| Json::from((0..4).map(|k| i16_at(exe, o(0x801B_1334) + 8 * i + 2 * k)).collect::<Vec<_>>())).collect();
    index.set("rings", rings);
    // sound definitions: 6-byte records u8 priority, u8 pan, u16 voice class, u16 volume
    let defs = |a: u32, n: usize| -> Json {
        Json::Arr(
            (0..n)
                .map(|i| {
                    Json::from(vec![
                        exe[o(a) + 6 * i] as i64,
                        exe[o(a) + 6 * i + 1] as i64,
                        u16_at(exe, o(a) + 6 * i + 2) as i64,
                        u16_at(exe, o(a) + 6 * i + 4) as i64,
                    ])
                })
                .collect(),
        )
    };
    let mut snd = Obj::new().with("COMMON_2.SND", defs(0x801B_31A8, 18)).with("COMMON_1.SND", defs(0x801B_3214, 51));
    snd.set("TITLE.SND", defs(0x801B_3348, 4));
    snd.set("CHASEL_1.SND", defs(0x801B_3360, 3));
    snd.set("CHASEL_2.SND", defs(0x801B_3374, 66));
    for c in 0..11 {
        snd.set(&format!("GOVER_{c:02}.SND"), defs(0x801B_3524, 1));
    }
    let st = [(0x3584, 4), (0x359C, 5), (0x35BC, 5), (0x35DC, 5), (0x35FC, 3), (0x3610, 6), (0x3634, 5), (0x3654, 6)];
    for (s, (a, n)) in st.into_iter().enumerate() {
        snd.set(&format!("STAGE_{s}.SND"), defs(0x801B_0000 + a, n));
    }
    let ch = [
        (0x3678, 25),
        (0x3710, 25),
        (0x37A8, 22),
        (0x382C, 22),
        (0x38B0, 23),
        (0x393C, 24),
        (0x39CC, 22),
        (0x3A50, 23),
        (0x3ADC, 21),
        (0x3B5C, 24),
        (0x3BEC, 18),
    ];
    for (c, (a, n)) in ch.into_iter().enumerate() {
        snd.set(&format!("CHARA_{c:02}.SND"), defs(0x801B_0000 + a, n));
    }
    index.set("sound_defs", snd);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tables are in address order and never overlap, and every byte of the executable a runtime was recorded
    /// reading (`tests/fixtures/runtime_reads.txt`) is in one.
    #[test]
    fn okt_tables_cover_the_recorded_reads() {
        let span = |&(_, e, n, o): &(&str, Elem, u32, u32)| (o, o + n * e.size() as u32);
        for w in TABLES.windows(2) {
            assert!(span(&w[0]).1 <= span(&w[1]).0, "{} overlaps {}", w[0].0, w[1].0);
        }
        let mut names: Vec<&str> = TABLES.iter().map(|t| t.0).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), TABLES.len());
        let reads = include_str!("../tests/fixtures/runtime_reads.txt");
        let mut n = 0;
        for l in reads.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let (a, b) = l.split_once(' ').unwrap();
            let (a, b) = (u32::from_str_radix(a, 16).unwrap(), u32::from_str_radix(b, 16).unwrap());
            for x in a..b {
                assert!(TABLES.iter().any(|t| (span(t).0..span(t).1).contains(&x)), "{x:08X} is in no table");
            }
            n += 1;
        }
        assert!(n > 200);
    }

    /// Every table but the text is the EXE's bytes at its origin, the text is the C strings by key, and the file reads
    /// back (on a synthetic EXE of the real one's size: letters, a NUL every 16 bytes, the lists pointing at them).
    #[test]
    fn okt_tables_are_the_exe_bytes() {
        let mut exe: Vec<u8> = (0..659_456u32).map(|i| if i % 16 == 15 { 0 } else { b'A' + (i % 26) as u8 }).collect();
        for &(_, table, n, _) in TEXT_LISTS {
            for k in 0..n {
                let o = (table - LOAD + 4 * k) as usize;
                exe[o..o + 4].copy_from_slice(&(LOAD + 0x800 + 16 * k).to_le_bytes());
            }
        }
        let t = okt::read(&tables_okt(&exe).unwrap()).unwrap();
        assert_eq!(t.len(), TABLES.len() + 1);
        for (tab, &(name, elem, count, origin)) in t.iter().zip(TABLES) {
            let o = (origin - LOAD) as usize;
            assert_eq!((tab.name.as_str(), tab.elem, tab.count, tab.origin), (name, elem, count, origin));
            assert_eq!(tab.bytes, &exe[o..o + count as usize * elem.size()]);
        }
        let text = t.last().unwrap();
        assert_eq!((text.name.as_str(), text.elem, text.count, text.origin), ("text.strings", Elem::Blob, text.bytes.len() as u32, 0));
        let mut entries = text.bytes.split(|&b| b == 0);
        let mut next = || (String::from_utf8(entries.next().unwrap().to_vec()).unwrap(), String::from_utf8(entries.next().unwrap().to_vec()).unwrap());
        for &(prefix, _, n, _) in TEXT_LISTS {
            for k in 0..n {
                // the string at 0x800 + 16k: its first letter from (0x800 + 16k) % 26 on, 15 letters
                let s: String = (0..15).map(|j| (b'A' + ((0x800 + 16 * k + j) % 26) as u8) as char).collect();
                assert_eq!(next(), (format!("{prefix}.{k}"), s));
            }
        }
        for &(key, addr) in TEXT {
            assert_eq!(next(), (key.to_string(), c_string(&exe, addr, true).unwrap()));
        }
        assert_eq!(entries.next(), Some(&[][..]));
        assert_eq!(entries.next(), None);
        assert!(tables_okt(&exe[..0x9_0000]).unwrap_err().contains("too short"));
    }

    /// A text: the C string up to its NUL, with `trim` its trailing spaces and line ends cut; not text or no NUL: Err.
    #[test]
    fn c_strings() {
        let exe = b"ROUND  \n\0 A B \0OK\x01\0NO END";
        assert_eq!(c_string(exe, LOAD, true).unwrap(), "ROUND");
        assert_eq!(c_string(exe, LOAD + 9, true).unwrap(), " A B");
        assert_eq!(c_string(exe, LOAD + 9, false).unwrap(), " A B ");
        assert!(c_string(exe, LOAD + 15, true).unwrap_err().contains("not text"));
        assert!(c_string(exe, LOAD + 19, true).unwrap_err().contains("no end"));
        assert!(c_string(exe, LOAD + 40, true).unwrap_err().contains("outside"));
        assert!(c_string(exe, 0x1000, true).unwrap_err().contains("outside"));
    }

    #[test]
    fn python_formats() {
        assert_eq!(phex(-5, 4), "-005");
        assert_eq!(phex(0x1A, 4), "001A");
        assert_eq!(phex(0x12345, 4), "12345");
        assert_eq!(hex(&[1, 0xAB], true), "01 ab");
        assert_eq!(slice(&[1, 2, 3], 2, 1), &[] as &[u8]);
    }
}
