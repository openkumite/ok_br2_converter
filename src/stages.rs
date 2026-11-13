//! The extra stage files: the split stages (`<ST>_base`, `<ST>_obj<k>`, `<ST>_opaque`, `<ST>_semi`), the
//! ordering-table files (`<ST>_ot`: one node per drawn unit in ordering-table order) and their second version
//! (`<ST>_ot_v2`, `<ST>_debris<k>_v2`: textured colours above 128 kept, written where they differ).

use crate::glb::{Glb, MeshBuilder, OrderedMesh, TriSink};
use crate::hmd::{Hmd, Prim};
use crate::json::Obj;
use crate::model::{self, Colors, Materials, Mats, Quad, add_floor, add_hmd, set_unlit, stp_page_rgba};
use crate::png::png_bytes;
use crate::vram::Vram;
use std::collections::HashMap;

/// The stages' winding sign of the full export (stage_objs.STAGE_WINDING).
pub const STAGE_WINDING: i64 = -1;

/// Objects each stage routine of table 0x801B14DC draws with its own matrix or on alternate frames.
pub fn moved(st: &str) -> &'static [i64] {
    match st {
        "ST1" => &[6, 7, 9, 11, 13, 15, 17, 19, 21],
        "ST2" => &[9],
        "ST4" => &[4],
        "ST6" => &[6, 7],
        "ST7" => &[8, 9],
        _ => &[],
    }
}
/// Objects never drawn.
pub fn skipped(st: &str) -> &'static [i64] {
    match st {
        "ST1" => &[8, 10, 12, 14, 16, 18, 20],
        "STC" => &[4, 5],
        _ => &[],
    }
}
fn drawn_last(st: &str) -> i64 {
    match st {
        "STC" => 6,
        _ => 4,
    }
}

/// `SkipMats`: no material (and no texture baked) for polygons that are left out.
struct SkipMats<'a, 'v> {
    mats: &'a mut Materials<'v>,
    route: &'a dyn Fn(&Prim) -> Option<usize>,
}

impl Mats for SkipMats<'_, '_> {
    fn get(&mut self, glb: &mut Glb, p: &Prim, textured: bool, unlit: bool) -> i64 {
        if (self.route)(p).is_none() { -1 } else { self.mats.get(glb, p, textured, unlit) }
    }
}

/// `add_routed`: a static model, each polygon into `route(p)` (None: left out).
fn add_routed(
    glb: &mut Glb,
    mats: &mut Materials,
    h: &Hmd,
    wsign: i64,
    route: &dyn Fn(&Prim) -> Option<usize>,
    sinks: &mut [&mut dyn TriSink],
) -> Result<(), String> {
    let mut sm = SkipMats { mats, route };
    add_hmd(glb, &mut sm, h, wsign, None, None, Colors::Clamped, &mut |p| route(p), sinks)
}

pub fn has_semi(hmds: &[(usize, &[u8])]) -> Result<bool, String> {
    for &(idx, data) in hmds {
        if idx == 2 && Hmd::parse(data)?.polys.iter().any(|(p, _)| p.semi) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn node(glb: &mut Glb, name: &str, mesh: usize) -> usize {
    glb.node(Obj::new().with("name", name).with("mesh", mesh))
}

/// `<ST>_base.glb` (or with `semi` `<ST>_opaque.glb`) and the moved objects' `<ST>_obj<k>.glb`.
pub fn export_split(hmds: &[(usize, &[u8])], vram: &Vram, name: &str, floor: &[Quad], semi: bool) -> Result<Vec<(String, Vec<u8>)>, String> {
    let (moved, skipped) = (moved(name), skipped(name));
    let mut files = Vec::new();
    let mut glb = Glb::new();
    let mut mats = Materials::new(vram, name);
    let mut nodes = Vec::new();
    if !floor.is_empty() {
        let mut mb = MeshBuilder::default();
        add_floor(&mut glb, &mut mats, &mut mb, floor);
        let n = format!("{name}_floor");
        let m = mb.build(&mut glb, &n, false);
        nodes.push(node(&mut glb, &n, m));
    }
    for &(idx, data) in hmds {
        let h = Hmd::parse(data)?;
        let wsign = model::static_winding(&h, STAGE_WINDING)?;
        let mut mb = MeshBuilder::default();
        if idx == 2 {
            let mut walls: Vec<MeshBuilder> = (0..4).map(|_| MeshBuilder::default()).collect();
            let route = |p: &Prim| -> Option<usize> {
                if semi && p.semi {
                    None
                } else if p.bone < 4 {
                    Some(1 + *model::pyi(&[0usize, 1, 2, 3], p.bone))
                } else if moved.contains(&p.bone) || skipped.contains(&p.bone) {
                    None
                } else {
                    Some(0)
                }
            };
            {
                let mut sinks: Vec<&mut dyn TriSink> = vec![&mut mb];
                for w in walls.iter_mut() {
                    sinks.push(w);
                }
                add_routed(&mut glb, &mut mats, &h, wsign, &route, &mut sinks)?;
            }
            for (k, w) in walls.iter().enumerate() {
                if !w.is_empty() {
                    let n = format!("{name}_wall{k}");
                    let m = w.build(&mut glb, &n, false);
                    nodes.push(node(&mut glb, &n, m));
                }
            }
            if !semi {
                for &k in moved {
                    let mut og = Glb::new();
                    let mut om = Materials::new(vram, name);
                    let mut ob = MeshBuilder::default();
                    add_routed(&mut og, &mut om, &h, wsign, &|p: &Prim| if p.bone == k { Some(0) } else { None }, &mut [&mut ob])?;
                    let n = format!("{name}_obj{k}");
                    let m = ob.build(&mut og, &n, false);
                    let on = node(&mut og, &n, m);
                    og.set_scene_nodes(vec![on]);
                    files.push((format!("{n}.glb"), og.save()));
                }
            }
        } else {
            add_routed(&mut glb, &mut mats, &h, wsign, &|_: &Prim| Some(0), &mut [&mut mb])?;
        }
        let mesh = mb.build(&mut glb, &format!("{name}_part{idx:02}"), false);
        if idx == 3 {
            nodes.push(
                glb.node(
                    Obj::new()
                        .with("name", format!("{name}_wall_debris"))
                        .with("mesh", mesh)
                        .with("extras", Obj::new().with("hiddenByDefault", true).with("toggle", "debris")),
                ),
            );
        } else {
            nodes.push(node(&mut glb, &format!("{name}_part{idx:02}"), mesh));
        }
    }
    let root = glb.node(Obj::new().with("name", name).with("children", nodes));
    glb.set_scene_nodes(vec![root]);
    files.insert(0, (format!("{name}_{}.glb", if semi { "opaque" } else { "base" }), glb.save()));
    Ok(files)
}

/// `<ST>_semi.glb`: the semi-transparent polygons of slot 2 that are drawn (texel alpha 255 = no STP bit,
/// 128 = STP bit, 0 = colour 0).
pub fn export_semi(hmds: &[(usize, &[u8])], vram: &Vram, name: &str) -> Result<Vec<u8>, String> {
    let skipped = skipped(name);
    let mut glb = Glb::new();
    let mut mats = Materials::new(vram, name);
    mats.stp = true;
    for &(idx, data) in hmds {
        if idx != 2 {
            continue;
        }
        let h = Hmd::parse(data)?;
        let mut bad: Vec<u32> =
            h.polys.iter().filter(|(p, _)| p.semi && !skipped.contains(&p.bone) && p.uvs.is_some() && (p.tpage >> 5) & 3 != 1).map(|(p, _)| p.tpage).collect();
        if !bad.is_empty() {
            bad.sort();
            bad.dedup();
            return Err(format!("{name}: semi-transparent polygons not additive: {bad:x?}"));
        }
        if h.polys.iter().any(|(p, _)| p.semi && !skipped.contains(&p.bone) && p.bone < 4) {
            return Err(format!("{name}: semi-transparent ring-wall polygons"));
        }
        let (mut mb, mut over) = (MeshBuilder::default(), MeshBuilder::default());
        let last = drawn_last(name);
        add_routed(
            &mut glb,
            &mut mats,
            &h,
            STAGE_WINDING,
            &|p: &Prim| {
                if !p.semi || skipped.contains(&p.bone) {
                    None
                } else if p.bone == last {
                    Some(1)
                } else {
                    Some(0)
                }
            },
            &mut [&mut mb, &mut over],
        )?;
        let mut ns = Vec::new();
        for (n, m) in [(format!("{name}_semi"), &mb), (format!("{name}_semi_over"), &over)] {
            if !m.is_empty() {
                let mi = m.build(&mut glb, &n, false);
                ns.push(node(&mut glb, &n, mi));
            }
        }
        glb.set_scene_nodes(ns);
    }
    Ok(glb.save())
}

/// The stage VRAM after the darkening TILE: STP bits of the CLUT block (256..351, 480..495) cleared.
pub fn runtime_vram(vram: &Vram) -> Vram {
    let mut rt = vram.clone();
    for y in 480..496 {
        for x in 256..352 {
            rt.mem[y * crate::vram::W + x] &= 0x7FFF;
        }
    }
    rt
}

/// `stage_ot.OtMats`: `Materials` on the runtime VRAM; semi-transparent polygons that still blend get `_stp`
/// materials.
struct OtMats<'v> {
    plain: Materials<'v>,
    rt: &'v Vram,
    name: String,
    stp: HashMap<(Option<(u32, u32)>, bool), i64>,
    stp_texels: HashMap<(u32, u32), bool>,
}

impl Mats for OtMats<'_> {
    fn get(&mut self, glb: &mut Glb, p: &Prim, textured: bool, unlit: bool) -> i64 {
        let (tpage, clut) = (p.tpage, p.clut);
        let blends = |s: &mut Self| {
            *s.stp_texels.entry((tpage & 0x19F, clut)).or_insert_with(|| stp_page_rgba(s.rt, tpage, clut).iter().skip(3).step_by(4).any(|&a| a == 128))
        };
        if !p.semi || (textured && !blends(self)) {
            return self.plain.get_tp(glb, tpage, clut, textured, unlit);
        }
        let key = (if textured { Some((tpage & 0x19F, clut)) } else { None }, unlit);
        if let Some(&m) = self.stp.get(&key) {
            return m;
        }
        let mut tex = None;
        if textured {
            let png = png_bytes(256, 256, &stp_page_rgba(self.rt, tpage, clut));
            tex = Some(glb.image_png(&png, &format!("{}_tp{:03x}_cl{:04x}_stp", self.name, tpage & 0x19F, clut)));
        }
        let nm = if textured { format!("tp{:03x}_cl{:04x}", tpage & 0x19F, clut) } else { "flat".to_string() };
        let mi = glb.material(&format!("{nm}{}_stp", if unlit { "_unlit" } else { "" }), tex);
        if unlit {
            set_unlit(glb, mi, false);
        }
        self.stp.insert(key, mi as i64);
        mi as i64
    }
}

/// `<ST>_ot.glb` (`stage_ot.export_ot`); `colors` = Clamped for v1, Unclamped for v2.
pub fn export_ot(ents: &[Option<Vec<u8>>], vram: &Vram, name: &str, colors: Colors) -> Result<Vec<u8>, String> {
    let rt = runtime_vram(vram);
    let mut glb = Glb::new();
    let mut mats = OtMats { plain: Materials::new(&rt, name), rt: &rt, name: name.to_string(), stp: HashMap::new(), stp_texels: HashMap::new() };
    let mut nodes = Vec::new();
    let floor = if ents.len() > 1 { model::floor_quads(ents[0].as_deref(), ents[1].as_deref()) } else { Vec::new() };
    if !floor.is_empty() {
        let mut mb = OrderedMesh::default();
        add_floor(&mut glb, &mut mats.plain, &mut mb, &floor);
        if !mb.runs.is_empty() {
            let n = format!("{name}_ot_floor");
            let m = mb.build(&mut glb, &n);
            nodes.push(node(&mut glb, &n, m));
        }
    }
    let data = ents[2].as_deref().ok_or("no slot 2")?;
    let mut h = Hmd::parse(data)?;
    let wsign = model::static_winding(&h, STAGE_WINDING)?;
    // the GPU's order: each object's polygons reversed
    h.polys.reverse();
    let mut bones: Vec<i64> = h.polys.iter().map(|(p, _)| p.bone).collect();
    bones.sort();
    bones.dedup();
    let mut meshes: Vec<OrderedMesh> = bones.iter().map(|_| OrderedMesh::default()).collect();
    {
        let mut sinks: Vec<&mut dyn TriSink> = meshes.iter_mut().map(|m| m as &mut dyn TriSink).collect();
        add_hmd(&mut glb, &mut mats, &h, wsign, None, None, colors, &mut |p| bones.binary_search(&p.bone).ok(), &mut sinks)?;
    }
    for (k, m) in bones.iter().zip(&meshes) {
        if !m.runs.is_empty() {
            let n = format!("{name}_ot_obj{k}");
            let mi = m.build(&mut glb, &n);
            nodes.push(node(&mut glb, &n, mi));
        }
    }
    let root = glb.node(Obj::new().with("name", format!("{name}_ot")).with("children", nodes));
    glb.set_scene_nodes(vec![root]);
    let l = glb.list("extensionsUsed");
    if !l.iter().any(|e| e.as_str() == Some("KHR_materials_unlit")) {
        l.push("KHR_materials_unlit".into());
    }
    Ok(glb.save())
}

/// `stage_ot_v2.export_debris`: the slot-3 pieces with the colours unclamped (`<name>_debris<k>_v2.glb`).
pub fn export_debris_v2(data: &[u8], vram: &Vram, name: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
    let h = Hmd::parse(data)?;
    let wsign = model::static_winding(&h, STAGE_WINDING)?;
    let mut tg = Glb::new();
    let mut tm = Materials::new(vram, name);
    model::debris_files(&mut tg, &mut tm, &h, vram, name, wsign, Colors::Unclamped, "_v2")
}
