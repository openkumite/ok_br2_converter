//! Characters and arenas as glTF (the meshes, the skeletons, the animations,
//! the floor and the static models), with Python's number semantics (`py::Num`) so every float has the same bits.

use crate::glb::{Corner, Drop, FLOAT, Glb, MeshBuilder, TriSink};
use crate::hmd::{Hmd, Prim};
use crate::json::{Json, Obj};
use crate::png::png_bytes;
use crate::py::{self, F, I, Num};
use crate::vram::Vram;
use std::collections::{BTreeMap, HashMap};

pub const SCALE: f64 = 1.0 / 300.0;
const TRANSLATION_SHIFT: i64 = 4;
pub const TICK: f64 = 1.0 / 60.0;

/// Python's `seq[i]` (negative counts from the end).
pub fn pyi<T>(v: &[T], i: i64) -> &T {
    if i < 0 { &v[(v.len() as i64 + i) as usize] } else { &v[i as usize] }
}

pub type V3 = [Num; 3];

pub fn conv(p: V3) -> V3 {
    [p[0] * F(SCALE), -p[1] * F(SCALE), -p[2] * F(SCALE)]
}

pub fn conv_dir(n: V3) -> V3 {
    let ln = py::sqrt(n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).or(F(1.0));
    [n[0] / ln, -n[1] / ln, -n[2] / ln]
}

fn mat_apply(m: &[i64; 9], v: [i64; 3]) -> V3 {
    let r = |a: usize| I(m[a] * v[0] + m[a + 1] * v[1] + m[a + 2] * v[2]) / F(4096.0);
    [r(0), r(3), r(6)]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: V3, b: V3) -> Num {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn i3(v: [i64; 3]) -> V3 {
    [I(v[0]), I(v[1]), I(v[2])]
}

/// A posed polygon corner: position, normal, coordinate (bone).
#[derive(Clone, Copy)]
pub struct GCorner {
    pub pos: V3,
    pub nrm: Option<V3>,
    pub bone: i64,
}

/// `hmd_geometry`: every polygon's corners in PS1 space. `bone_pos` = per-coordinate joint positions (characters:
/// the ANI skeleton), default the HMD's coordinate translations.
pub fn hmd_geometry(h: &Hmd, bone_pos: Option<&[V3]>) -> Result<Vec<Vec<GCorner>>, String> {
    let mut world: Vec<(&[i64; 9], V3)> = Vec::new();
    for (i, c) in h.coords.iter().enumerate() {
        let t = match bone_pos {
            Some(bp) => bp[i],
            None => [I(c.t[0]) / I(TRANSLATION_SHIFT), I(c.t[1]) / I(TRANSLATION_SHIFT), I(c.t[2]) / I(TRANSLATION_SHIFT)],
        };
        world.push((&c.m, t));
    }
    const IDENT: [i64; 9] = [4096, 0, 0, 0, 4096, 0, 0, 0, 4096];
    if world.is_empty() {
        world.push((&IDENT, [I(0); 3]));
    }
    let pick = |bone: i64| if 0 <= bone && (bone as usize) < world.len() { world[bone as usize] } else { world[0] };
    let xform = |bone: i64, v: [i64; 3]| {
        let (m, t) = pick(bone);
        let r = mat_apply(m, v);
        [r[0] + t[0], r[1] + t[1], r[2] + t[2]]
    };
    let xdir = |bone: i64, n: [i64; 3]| mat_apply(pick(bone).0, n);

    let mut work_v: HashMap<(u32, i64), (V3, i64)> = HashMap::new();
    let mut work_n: HashMap<(u32, i64), V3> = HashMap::new();
    for c in &h.calcs {
        let hdr = &h.headers[&c.hidx];
        for i in 0..c.vc {
            work_v.insert((c.hidx, (c.vd + i) as i64), (xform(c.bone, h.svec(hdr[1], c.vs + i)?), c.bone));
        }
        for i in 0..c.nc {
            work_n.insert((c.hidx, (c.nd + i) as i64), xdir(c.bone, h.svec(hdr[3], c.ns + i)?));
        }
    }
    let shared: Vec<(u32, &Vec<u32>)> = h.header_order.iter().map(|k| (*k, &h.headers[k])).filter(|(_, v)| v.len() == 5).collect();
    let work_ranges: Vec<(i64, u32)> = shared.iter().map(|(k, hdr)| (hdr[2] as i64 * 4, *k)).collect();
    let work_nranges: Vec<(i64, u32)> = shared.iter().map(|(k, hdr)| (hdr[4] as i64 * 4, *k)).collect();

    let mut out = Vec::with_capacity(h.polys.len());
    for (p, hidx) in &h.polys {
        let hdr = &h.headers[hidx];
        let mut corners = Vec::with_capacity(p.verts.len());
        for (k, &vi) in p.verts.iter().enumerate() {
            let ni = p.norms.as_ref().map(|n| n[k]);
            if p.shared {
                let (pos, b) = work_v.get(&(*hidx, vi as i64)).copied().unwrap_or(([I(0); 3], 0));
                let nrm = ni.and_then(|ni| work_n.get(&(*hidx, ni as i64)).copied());
                corners.push(GCorner { pos, nrm, bone: b });
            } else {
                let addr = hdr[1] as i64 * 4 + vi as i64 * 8;
                let mut found = None;
                for &(base, hk) in &work_ranges {
                    if addr >= base
                        && let Some(&e) = work_v.get(&(hk, (addr - base).div_euclid(8)))
                    {
                        found = Some(e);
                        break;
                    }
                }
                let (pos, b) = match found {
                    Some(e) => e,
                    None => (xform(p.bone, h.svec(hdr[1], vi as u32)?), p.bone),
                };
                let mut nrm = None;
                if let Some(ni) = ni
                    && hdr.len() >= 3
                {
                    let naddr = hdr[2] as i64 * 4 + ni as i64 * 8;
                    let mut hit = false;
                    for &(base, hk) in &work_nranges {
                        if naddr >= base
                            && let Some(&e) = work_n.get(&(hk, (naddr - base).div_euclid(8)))
                        {
                            nrm = Some(e);
                            hit = true;
                            break;
                        }
                    }
                    if !hit && naddr + 6 <= h.d.len() as i64 {
                        nrm = Some(xdir(p.bone, h.svec(hdr[2], ni as u32)?));
                    }
                }
                corners.push(GCorner { pos, nrm, bone: b });
            }
        }
        out.push(corners);
    }
    Ok(out)
}

/// Whether lit polygons are wound so that cross(v1 - v0, v2 - v0) agrees with their normals.
pub fn winding_sign(geoms: &[Vec<GCorner>]) -> i64 {
    let mut score = 0;
    for cs in geoms {
        let Some(n) = cs[0].nrm else { continue };
        let f = cross(sub(cs[1].pos, cs[0].pos), sub(cs[2].pos, cs[0].pos));
        score += if dot(f, n).gt(I(0)) { 1 } else { -1 };
    }
    if score >= 0 { 1 } else { -1 }
}

/// Where `add_hmd` gets a polygon's material (`Materials.get` and the variants of stage_objs / stage_ot).
pub trait Mats {
    fn get(&mut self, glb: &mut Glb, p: &Prim, textured: bool, unlit: bool) -> i64;
}

/// `convert.Materials`: one material per (texture page, CLUT, lighting).
pub struct Materials<'v> {
    pub vram: &'v Vram,
    /// `StpVram`: pages decoded with the STP bit in alpha
    pub stp: bool,
    pub prefix: String,
    cache: HashMap<(Option<(u32, u32)>, bool), i64>,
    tex_cache: HashMap<(u32, u32), usize>,
    pub missing: std::collections::HashSet<(u32, u32)>,
}

impl<'v> Materials<'v> {
    pub fn new(vram: &'v Vram, prefix: &str) -> Self {
        Materials { vram, stp: false, prefix: prefix.to_string(), cache: HashMap::new(), tex_cache: HashMap::new(), missing: Default::default() }
    }

    pub fn get_tp(&mut self, glb: &mut Glb, tpage: u32, clut: u32, textured: bool, unlit: bool) -> i64 {
        let key = (if textured { Some((tpage & 0x19F, clut)) } else { None }, unlit);
        if let Some(&m) = self.cache.get(&key) {
            return m;
        }
        let mi = if textured {
            let tkey = (tpage & 0x19F, clut);
            let tex = match self.tex_cache.get(&tkey) {
                Some(&t) => t,
                None => {
                    let rgba = if self.stp { stp_page_rgba(self.vram, tpage, clut) } else { self.vram.page_rgba(tpage, clut) };
                    if !rgba.iter().skip(3).step_by(4).any(|&a| a != 0) {
                        self.missing.insert(tkey);
                    }
                    let name = format!("{}_tp{:03x}_cl{:04x}", self.prefix, tpage & 0x19F, clut);
                    let t = glb.image_png(&png_bytes(256, 256, &rgba), &name);
                    self.tex_cache.insert(tkey, t);
                    t
                }
            };
            glb.material(&format!("tp{:03x}_cl{:04x}{}", tpage & 0x19F, clut, if unlit { "_unlit" } else { "" }), Some(tex))
        } else {
            glb.material(&format!("flat{}", if unlit { "_unlit" } else { "" }), None)
        };
        if unlit {
            set_unlit(glb, mi, true);
        }
        self.cache.insert(key, mi as i64);
        mi as i64
    }
}

/// `materials[mi]['extensions'] = {'KHR_materials_unlit': {}}` (and, with `used`, the extensionsUsed entry).
pub fn set_unlit(glb: &mut Glb, mi: usize, used: bool) {
    glb.material_mut(mi).set("extensions", Obj::new().with("KHR_materials_unlit", Obj::new()));
    if used {
        let l = glb.list("extensionsUsed");
        if !l.iter().any(|e| e.as_str() == Some("KHR_materials_unlit")) {
            l.push("KHR_materials_unlit".into());
        }
    }
}

impl Mats for Materials<'_> {
    fn get(&mut self, glb: &mut Glb, p: &Prim, textured: bool, unlit: bool) -> i64 {
        self.get_tp(glb, p.tpage, p.clut, textured, unlit)
    }
}

/// One material for every polygon (the shadow proxy).
pub struct OneMat(pub i64);

impl Mats for OneMat {
    fn get(&mut self, _glb: &mut Glb, _p: &Prim, _t: bool, _u: bool) -> i64 {
        self.0
    }
}

/// `stage_objs.stp_page_rgba`: `page_rgba` with the STP bit in alpha (0 = colour 0, 128 = STP set, 255 = clear).
pub fn stp_page_rgba(vram: &Vram, tpage: u32, clut: u32) -> Vec<u8> {
    use crate::vram::{H, W};
    let mut rgba = vram.page_rgba(tpage, clut);
    let (px, py, mode) = ((tpage as usize & 0xF) * 64, ((tpage as usize >> 4) & 1) * 256, (tpage >> 7) & 3);
    let (cx, cy) = ((clut as usize & 0x3F) * 16, (clut as usize >> 6) & 0x1FF);
    let mem = &vram.mem;
    for v in 0..256 {
        let row = ((py + v) % H) * W;
        for u in 0..256usize {
            let c = match mode {
                0 => mem[cy * W + cx + ((mem[row + (px + (u >> 2)) % W] >> ((u & 3) * 4)) & 0xF) as usize],
                1 => mem[cy * W + cx + ((mem[row + (px + (u >> 1)) % W] >> ((u & 1) * 8)) & 0xFF) as usize],
                _ => mem[row + (px + u) % W],
            };
            if c & 0x8000 != 0 && c != 0 {
                rgba[4 * (v * 256 + u) + 3] = 128;
            }
        }
    }
    rgba
}

/// How `add_hmd` treats colours: textured ones / 128 clamped at 1.0 or not (the second ordering-table files).
#[derive(Clone, Copy, PartialEq)]
pub enum Colors {
    Clamped,
    Unclamped,
    None,
}

/// `add_hmd`: every polygon of `h` as triangles; `route(p)` picks the sink (None: dropped, after its material).
/// `mats` is asked for each polygon's material first, (the order fixes the materials' numbering).
pub fn add_hmd(
    glb: &mut Glb,
    mats: &mut dyn Mats,
    h: &Hmd,
    wsign: i64,
    skinned: Option<&[i64]>,
    bone_pos: Option<&[V3]>,
    colors: Colors,
    route: &mut dyn FnMut(&Prim) -> Option<usize>,
    sinks: &mut [&mut dyn TriSink],
) -> Result<(), String> {
    let geoms = hmd_geometry(h, bone_pos)?;
    for ((p, _), cs) in h.polys.iter().zip(&geoms) {
        let target = route(p);
        let textured = p.uvs.is_some();
        let unlit = p.norms.is_none();
        let mat = mats.get(glb, p, textured, unlit);
        let mut corners: Vec<Corner> = Vec::with_capacity(cs.len());
        for (k, c) in cs.iter().enumerate() {
            let uv = p.uvs.as_ref().map(|u| [I(u[k].0 as i64) / F(256.0), I(u[k].1 as i64) / F(256.0)]);
            let col = match (&p.color, colors) {
                (Some(cols), Colors::Clamped | Colors::Unclamped) => {
                    let c = if cols.len() > 1 { cols[k] } else { cols[0] };
                    let s = F(if textured { 128.0 } else { 255.0 });
                    Some(c.map(|x| {
                        let v = I(x as i64) / s;
                        if colors == Colors::Clamped { py::min2(F(1.0), v) } else { v }
                    }))
                }
                _ => None,
            };
            corners.push(Corner { pos: conv(c.pos), nrm: c.nrm.map(conv_dir), uv, col, joint: skinned.map(|s| *pyi(s, c.bone)) });
        }
        let tris: &[[usize; 3]] = if p.quad { &[[0, 1, 2], [1, 3, 2]] } else { &[[0, 1, 2]] };
        for t in tris {
            let mut tri = [corners[t[0]].clone(), corners[t[1]].clone(), corners[t[2]].clone()];
            if tri.iter().any(|x| x.nrm.is_none()) {
                let f = cross(sub(cs[t[1]].pos, cs[t[0]].pos), sub(cs[t[2]].pos, cs[t[0]].pos));
                let f = f.map(|x| x * I(wsign));
                let n = if f.iter().any(|x| x.truthy()) { conv_dir(f) } else { [F(0.0), F(1.0), F(0.0)] };
                for x in tri.iter_mut() {
                    x.nrm = Some(n);
                }
            }
            if let Some(si) = target {
                let [a, b, c] = &tri;
                if wsign > 0 { sinks[si].tri(mat, [a, b, c]) } else { sinks[si].tri(mat, [a, c, b]) }
            }
        }
    }
    Ok(())
}

pub fn s10(v: u32) -> i64 {
    let v = (v & 1023) as i64;
    if v & 512 != 0 { v - 1024 } else { v }
}

fn u16_at(d: &[u8], o: usize) -> Result<u16, String> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(|| format!("read past the end at {o:#x}"))
}
fn i16_at(d: &[u8], o: usize) -> Result<i64, String> {
    Ok(u16_at(d, o)? as i16 as i64)
}
fn u32_at(d: &[u8], o: usize) -> Result<u32, String> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| format!("read past the end at {o:#x}"))
}

const NODE_NAMES: [&str; 19] = [
    "root",
    "body",
    "head",
    "chest",
    "pelvis",
    "thigh.R",
    "shin.R",
    "foot.R",
    "thigh.L",
    "shin.L",
    "foot.L",
    "upperarm.R",
    "forearm.R",
    "hand.R",
    "upperarm.L",
    "forearm.L",
    "hand.L",
    "hip.R",
    "hip.L",
];

pub type M3 = [[Num; 3]; 3];

fn i3x3() -> M3 {
    [[F(1.0), F(0.0), F(0.0)], [F(0.0), F(1.0), F(0.0)], [F(0.0), F(0.0), F(1.0)]]
}

fn mm(a: &M3, b: &M3) -> M3 {
    let mut r = [[I(0); 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = py::sum((0..3).map(|k| a[i][k] * b[k][j]));
        }
    }
    r
}

fn mv(a: &M3, v: V3) -> V3 {
    [0, 1, 2].map(|i| py::sum((0..3).map(|k| a[i][k] * v[k])))
}

fn mv1(a: &M3, v: V3) -> Num {
    py::sum((0..3).map(|k| a[1][k] * v[k]))
}

fn ang(x: Num) -> Num {
    x * F(std::f64::consts::PI) / F(2048.0)
}

fn euler_zyx(ax: Num, ay: Num, az: Num) -> M3 {
    let (ax, ay, az) = (ang(ax), ang(ay), ang(az));
    let (cx, sx, cy, sy, cz, sz) = (py::cos(ax), py::sin(ax), py::cos(ay), py::sin(ay), py::cos(az), py::sin(az));
    let rx = [[I(1), I(0), I(0)], [I(0), cx, -sx], [I(0), sx, cx]];
    let ry = [[cy, I(0), sy], [I(0), I(1), I(0)], [-sy, I(0), cy]];
    let rz = [[cz, -sz, I(0)], [sz, cz, I(0)], [I(0), I(0), I(1)]];
    mm(&mm(&rz, &ry), &rx)
}

fn swing_twist(x: i64, y: i64, z: i64, twist: i64) -> M3 {
    let n = py::sqrt(I(x * x + y * y + z * z)).or(F(1.0));
    let (dx, dy, dz) = (I(x) / n, I(y) / n, I(z) / n);
    let h = py::sqrt(dx * dx + dz * dz);
    let (hx, hz) = if h.gt(F(1e-9)) { (dx / h, dz / h) } else { (F(1.0), F(0.0)) };
    let k = F(1.0) - dy;
    let swing = [[dy + k * hz * hz, dx, -k * hx * hz], [-dx, dy, -dz], [-k * hx * hz, dz, dy + k * hx * hx]];
    let t = ang(I(twist));
    let (c, s) = (py::cos(t), py::sin(t));
    mm(&swing, &[[c, I(0), -s], [I(0), I(1), I(0)], [s, I(0), c]])
}

fn rot_y(a: i64) -> M3 {
    let t = ang(I(a));
    let (c, s) = (py::cos(t), py::sin(t));
    [[c, I(0), s], [I(0), I(1), I(0)], [-s, I(0), c]]
}

fn mat_to_quat(m: &M3) -> [Num; 4] {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let q = if tr.gt(I(0)) {
        let s = py::sqrt(tr + F(1.0)) * I(2);
        [(m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s, F(0.25) * s]
    } else if m[0][0].gt(m[1][1]) && m[0][0].gt(m[2][2]) {
        let s = py::sqrt(F(1.0) + m[0][0] - m[1][1] - m[2][2]) * I(2);
        [F(0.25) * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s, (m[2][1] - m[1][2]) / s]
    } else if m[1][1].gt(m[2][2]) {
        let s = py::sqrt(F(1.0) + m[1][1] - m[0][0] - m[2][2]) * I(2);
        [(m[0][1] + m[1][0]) / s, F(0.25) * s, (m[1][2] + m[2][1]) / s, (m[0][2] - m[2][0]) / s]
    } else {
        let s = py::sqrt(F(1.0) + m[2][2] - m[0][0] - m[1][1]) * I(2);
        [(m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, F(0.25) * s, (m[1][0] - m[0][1]) / s]
    };
    let n = py::sqrt(py::sum(q.iter().map(|&x| x * x))).or(F(1.0));
    q.map(|x| x / n)
}

fn half_rot(m: &M3) -> M3 {
    let [mut x, mut y, mut z, mut w] = mat_to_quat(m);
    if w.lt(I(0)) {
        (x, y, z, w) = (-x, -y, -z, -w);
    }
    let a = I(2) * py::acos(py::max2(F(-1.0), py::min2(F(1.0), w)));
    let s = py::sin(a / I(2)).or(F(1.0));
    let ax = if a.gt(F(1e-6)) { [x / s, y / s, z / s] } else { [F(0.0), F(1.0), F(0.0)] };
    let h = a / I(2);
    let sh = py::sin(h / I(2));
    let (qx, qy, qz, qw) = (ax[0] * sh, ax[1] * sh, ax[2] * sh, py::cos(h / I(2)));
    [
        [I(1) - I(2) * (qy * qy + qz * qz), I(2) * (qx * qy - qz * qw), I(2) * (qx * qz + qy * qw)],
        [I(2) * (qx * qy + qz * qw), I(1) - I(2) * (qx * qx + qz * qz), I(2) * (qy * qz - qx * qw)],
        [I(2) * (qx * qz - qy * qw), I(2) * (qy * qz + qx * qw), I(1) - I(2) * (qx * qx + qy * qy)],
    ]
}

/// The in-game skeleton of one form, from `<CODE><n>P.ANI` (`convert.Skeleton`).
pub struct Skeleton {
    pub node_of: Vec<i64>,
    pub parent: Vec<i64>,
    pub offset: Vec<[i64; 3]>,
    pub base_rot: Vec<M3>,
    pub connector_of: BTreeMap<usize, i64>,
    pub pos: Vec<[i64; 3]>,
}

impl Skeleton {
    pub fn new(ani: &[u8], form: usize, ncoords: usize) -> Result<Skeleton, String> {
        let table = u32_at(ani, 24)? as usize;
        let mut desc = [0usize; 7];
        for (k, d) in desc.iter_mut().enumerate() {
            *d = u32_at(ani, table + 28 * form + 4 * k)? as usize;
        }
        let mut node_of = Vec::with_capacity(ncoords);
        for c in 0..ncoords {
            node_of.push(i16_at(ani, desc[1] + 2 * c)?);
        }
        let mut nn = node_of.iter().copied().max().ok_or("model without coordinates")? + 1;
        let mut parent32 = Vec::with_capacity(32);
        for n in 0..32 {
            parent32.push(i16_at(ani, desc[0] + 2 * n)?);
        }
        let mut m = nn;
        for n in 0..nn.max(0) as usize {
            if parent32[n] >= 0 {
                m = m.max(parent32[n] + 1);
            }
        }
        nn = m;
        let nn = nn as usize;
        let mut parent: Vec<i64> = parent32[..nn.min(32)].to_vec();
        let mut offset = vec![[0i64; 3]];
        let mut base_rot = vec![i3x3()];
        for i in 0..nn - 1 {
            let w = u32_at(ani, desc[2] + 4 * i)?;
            offset.push([s10(w), s10(w >> 10), s10(w >> 20)]);
        }
        for i in 0..nn - 1 {
            let w = u32_at(ani, desc[4] + 4 * i)?;
            base_rot.push(euler_zyx(I(s10(w) * 4), I(s10(w >> 10) * 4), I(s10(w >> 20) * 4)));
        }
        let mut connector_of = BTreeMap::new();
        let mut a = desc[5];
        for n in 19..nn {
            if a + 4 > ani.len() {
                break;
            }
            let (hdr, cnt) = (u16_at(ani, a)?, i16_at(ani, a + 2)?);
            if !(0..=32).contains(&cnt) {
                break;
            }
            let kind = hdr & 0xFF;
            a += 4 + 2 * cnt as usize;
            if kind == 1 && parent[n] > 0 {
                connector_of.insert(n, parent[n]);
                parent[n] = *pyi(&parent, parent[n]);
            }
        }
        let mut sk = Skeleton { node_of, parent, offset, base_rot, connector_of, pos: Vec::new() };
        let mut pos: Vec<Option<[i64; 3]>> = vec![None; nn];
        for n in 0..nn {
            sk.world(n, &mut pos, 0)?;
        }
        sk.pos = pos.into_iter().map(Option::unwrap).collect();
        Ok(sk)
    }

    fn world(&self, n: usize, pos: &mut Vec<Option<[i64; 3]>>, depth: usize) -> Result<[i64; 3], String> {
        if depth > 64 {
            return Err("skeleton parent loop".into());
        }
        if let Some(p) = pos[n] {
            return Ok(p);
        }
        let p = self.parent[n];
        let base = if 0 <= p && (p as usize) < pos.len() && p as usize != n { self.world(p as usize, pos, depth + 1)? } else { [0; 3] };
        let o = self.offset[n];
        let r = [base[0] + o[0], base[1] + o[1], base[2] + o[2]];
        pos[n] = Some(r);
        Ok(r)
    }

    pub fn name(&self, n: usize) -> String {
        if n < NODE_NAMES.len() { NODE_NAMES[n].to_string() } else { format!("extra{n:02}") }
    }

    pub fn nn(&self) -> usize {
        self.parent.len()
    }
}

/// Body spheres from the ANI (`convert.collision_spheres`).
pub fn collision_spheres(ani: &[u8], form: usize, sk: &Skeleton) -> Result<Vec<Json>, String> {
    let sec = u32_at(ani, 32)? as usize;
    let mut out = Vec::new();
    for (setname, k) in [("A", form), ("B", 2 + form)] {
        let count = u16_at(ani, sec + 2 * k)? as usize;
        let tab = u32_at(ani, sec + 8 + 4 * k)? as usize;
        for i in 0..count {
            let (grp, node, rad) =
                (*ani.get(tab + 4 * i).ok_or("sphere table past the end")?, *ani.get(tab + 4 * i + 1).ok_or("sphere")?, u16_at(ani, tab + 4 * i + 2)?);
            if (node as usize) < sk.nn() {
                out.push(Json::Obj(
                    Obj::new().with("set", setname).with("group", grp).with("bone", sk.name(node as usize)).with("radius", py::round(rad as f64 * SCALE, 4)),
                ));
            }
        }
    }
    Ok(out)
}

fn vrows(v: &[V3]) -> Vec<Vec<Num>> {
    v.iter().map(|x| x.to_vec()).collect()
}

/// `export_character`: the skinned model, its shadow proxy and every animation. Returns the .glb and the
/// manifest info.
pub fn export_character(
    hmd_data: &[u8],
    ani: &[u8],
    form: usize,
    vram: &Vram,
    name: &str,
    act: Option<&[u8]>,
    proxy: Option<&[u8]>,
    cmn_act: Option<&[u8]>,
) -> Result<(Vec<u8>, Obj), String> {
    let h = Hmd::parse(hmd_data)?;
    let sk = Skeleton::new(ani, form, h.coords.len())?;
    let bone_pos: Vec<V3> = (0..h.coords.len()).map(|c| i3(*pyi(&sk.pos, sk.node_of[c]))).collect();
    let mut glb = Glb::new();
    let mut mats = Materials::new(vram, name);
    let (mut body, mut alt) = (MeshBuilder::default(), MeshBuilder::default());
    let wsign = winding_sign(&hmd_geometry(&h, Some(&bone_pos))?);
    // coordinates 15/16 are the alternate (open) hands; they share the fists' joints
    add_hmd(
        &mut glb,
        &mut mats,
        &h,
        wsign,
        Some(&sk.node_of),
        Some(&bone_pos),
        Colors::Clamped,
        &mut |p| Some(if p.bone == 15 || p.bone == 16 { 1 } else { 0 }),
        &mut [&mut body, &mut alt],
    )?;
    let nn = sk.pos.len();
    let mut nodes = Vec::with_capacity(nn);
    for n in 0..nn {
        nodes.push(glb.node(Obj::new().with("name", sk.name(n)).with("translation", conv(i3(sk.offset[n])).to_vec())));
    }
    for n in 0..nn {
        let kids: Vec<usize> = (0..nn).filter(|&k| sk.parent[k] == n as i64 && k != n).map(|k| nodes[k]).collect();
        if !kids.is_empty() {
            glb.node_mut(nodes[n]).set("children", kids);
        }
    }
    let ibm: Vec<Vec<Num>> = (0..nn)
        .map(|n| {
            let [tx, ty, tz] = conv(i3(sk.pos[n]));
            vec![I(1), I(0), I(0), I(0), I(0), I(1), I(0), I(0), I(0), I(0), I(1), I(0), -tx, -ty, -tz, I(1)]
        })
        .collect();
    let acc = glb.accessor(&ibm, FLOAT, "MAT4", None, false);
    glb.g.set(
        "skins",
        vec![Json::Obj(
            Obj::new().with("joints", nodes.clone()).with("skeleton", nodes[0]).with("name", format!("{name}_skeleton")).with("inverseBindMatrices", acc),
        )],
    );
    let m = body.build(&mut glb, name, true);
    let mut meshes = vec![glb.node(Obj::new().with("name", name).with("mesh", m).with("skin", 0))];
    if !alt.is_empty() {
        let n = format!("{name}_alt_hands");
        let m = alt.build(&mut glb, &n, true);
        meshes.push(
            glb.node(Obj::new().with("name", n).with("mesh", m).with("skin", 0).with("extras", Obj::new().with("hiddenByDefault", true).with("toggle", "alt"))),
        );
    }
    if let Some(pd) = proxy.filter(|p| !p.is_empty()) {
        // the low-poly stand-in the game renders for the drop shadow (HMD slots 1/3): coordinates 0-14 are the main
        // model's bones, so it shares the skeleton
        let ph = Hmd::parse(pd)?;
        let mut pmb = MeshBuilder::default();
        let pmat = glb.material("shadow_proxy", None);
        let pbr = Obj::new().with("baseColorFactor", vec![0.42, 0.46, 0.56, 0.9]).with("metallicFactor", 0.0).with("roughnessFactor", 1.0);
        let mo = glb.material_mut(pmat);
        mo.set("alphaMode", "BLEND");
        mo.set("doubleSided", true);
        mo.set("pbrMetallicRoughness", pbr);
        let pbone_pos: Vec<V3> = (0..ph.coords.len()).map(|c| if c < bone_pos.len() { bone_pos[c] } else { [I(0); 3] }).collect();
        let ws = winding_sign(&hmd_geometry(&ph, Some(&pbone_pos))?);
        add_hmd(&mut glb, &mut OneMat(pmat as i64), &ph, ws, Some(&sk.node_of), Some(&pbone_pos), Colors::None, &mut |_| Some(0), &mut [&mut pmb])?;
        let n = format!("{name}_shadow_proxy");
        let m = pmb.build(&mut glb, &n, true);
        meshes.push(glb.node(
            Obj::new().with("name", n).with("mesh", m).with("skin", 0).with("extras", Obj::new().with("hiddenByDefault", true).with("toggle", "shadow")),
        ));
    }
    let mut kids = vec![nodes[0]];
    kids.extend(&meshes);
    let root = glb.node(Obj::new().with("name", format!("{name}_root")).with("children", kids));
    glb.set_scene_nodes(vec![root]);
    let zero = vec![[I(0); 3]; h.coords.len()];
    let mut verts: Vec<(V3, i64)> = Vec::new();
    for ((p, _), cs) in h.polys.iter().zip(hmd_geometry(&h, Some(&zero))?) {
        if p.bone == 15 || p.bone == 16 {
            continue;
        }
        for c in cs {
            verts.push((c.pos, *pyi(&sk.node_of, c.bone)));
        }
    }
    let mut nanims = 0;
    if let Some(act) = act.filter(|a| !a.is_empty()) {
        nanims += add_animations(&mut glb, &sk, act, &nodes, &verts, "anim", false)?;
    }
    if let Some(cmn) = cmn_act.filter(|a| !a.is_empty()) {
        nanims += add_animations(&mut glb, &sk, cmn, &nodes, &verts, "cmn", true)?;
    }
    let info = Obj::new()
        .with("bones", nn)
        .with("polys", h.polys.len())
        .with("animations", nanims)
        .with("spheres", collision_spheres(ani, form, &sk)?)
        .with("missing_textures", mats.missing.len());
    Ok((glb.save(), info))
}

/// `read_act`: animation index → [(time in ticks, pose offset)].
fn read_act(act: &[u8], pose_len: usize) -> Result<BTreeMap<usize, Vec<(i64, usize)>>, String> {
    let count = u32_at(act, 4)? as usize;
    let mut anims = BTreeMap::new();
    for i in 0..count {
        let base = u32_at(act, 8 + 4 * i)? as usize;
        if base == 0 || base + 8 > act.len() {
            continue;
        }
        let (mut keys, mut a, mut t) = (Vec::new(), base + 4, 0i64);
        while a + 4 <= act.len() && keys.len() < 256 {
            let (off, dur) = (u16_at(act, a)? as usize, u16_at(act, a + 2)? as i64);
            a += 4;
            keys.push((t, base + off));
            t += dur;
            if dur == 0 {
                break;
            }
        }
        if !keys.is_empty() && keys.iter().all(|&(_, p)| p + pose_len <= act.len()) {
            anims.insert(i, keys);
        }
    }
    Ok(anims)
}

fn pose_local(act: &[u8], p: usize, sk: &Skeleton, short: bool) -> Result<Vec<M3>, String> {
    let nn = sk.nn();
    let mut loc = vec![i3x3(); nn];
    for n in 1..nn.min(17) {
        let o = p + 20 + 6 * (n - 1);
        let (w, tw) = (u32_at(act, o)?, i16_at(act, o + 4)?);
        loc[n] = mm(&sk.base_rot[n], &swing_twist(s10(w), s10(w >> 10), s10(w >> 20), tw));
    }
    for (n, o) in [(17, 116), (18, 118)] {
        if n < nn && !short {
            loc[n] = rot_y(-i16_at(act, p + o)?);
        }
    }
    for (&n, &src) in &sk.connector_of {
        loc[n] = half_rot(pyi(&loc, src));
    }
    Ok(loc)
}

type Stretch = BTreeMap<usize, [i64; 3]>;

fn pose_stretch(act: &[u8], p: usize, prev: &Stretch) -> Stretch {
    let mut cur = prev.clone();
    let mut a = p + 120;
    for _ in 0..32 {
        if a + 4 > act.len() || act[a] == 0xFF {
            break;
        }
        let (idx, d) = (act[a] as usize, [act[a + 1] as i8 as i64, act[a + 2] as i8 as i64, act[a + 3] as i8 as i64]);
        a += 4;
        if idx <= 17 {
            cur.insert(idx + 1, d);
        }
    }
    cur
}

fn pose_world(sk: &Skeleton, loc: &[M3], stretch: &Stretch) -> Vec<(M3, V3)> {
    let nn = sk.nn();
    let mut w: Vec<Option<(M3, V3)>> = vec![None; nn];
    w[0] = Some((i3x3(), [F(0.0); 3]));
    fn go(n: i64, sk: &Skeleton, loc: &[M3], st: &Stretch, w: &mut Vec<Option<(M3, V3)>>) -> (M3, V3) {
        let ni = if n < 0 { (w.len() as i64 + n) as usize } else { n as usize };
        if let Some(e) = w[ni] {
            return e;
        }
        let (pr, pp) = go(sk.parent[ni], sk, loc, st, w);
        let off = match st.get(&ni) {
            Some(d) => [sk.offset[ni][0] + d[0], sk.offset[ni][1] + d[1], sk.offset[ni][2] + d[2]],
            None => sk.offset[ni],
        };
        let r = mv(&pr, i3(off));
        let e = (mm(&pr, &loc[ni]), [pp[0] + r[0], pp[1] + r[1], pp[2] + r[2]]);
        w[ni] = Some(e);
        e
    }
    for n in 0..nn {
        go(n as i64, sk, loc, stretch, &mut w);
    }
    w.into_iter().map(Option::unwrap).collect()
}

fn lowest(w: &[(M3, V3)], verts: &[(V3, i64)]) -> Num {
    let mut best: Option<Num> = None;
    for &(v, n) in verts {
        let e = pyi(w, n);
        let y = e.1[1] + mv1(&e.0, v);
        best = Some(match best {
            None => y,
            Some(b) => py::max2(b, y),
        });
    }
    best.unwrap_or(I(0))
}

/// `add_animations`: one glTF animation per ACT entry; the body root moves per key so the mesh's lowest point
/// keeps its rest height. Returns the number written.
fn add_animations(glb: &mut Glb, sk: &Skeleton, act: &[u8], joint_nodes: &[usize], verts: &[(V3, i64)], prefix: &str, short: bool) -> Result<usize, String> {
    let anims = read_act(act, if short { 116 } else { 120 })?;
    let nn = sk.nn();
    let rest_low = lowest(&pose_world(sk, &vec![i3x3(); nn], &Stretch::new()), verts);
    let c: M3 = [[I(1), I(0), I(0)], [I(0), I(-1), I(0)], [I(0), I(0), I(-1)]];
    let mut written = 0;
    for (idx, keys) in &anims {
        let mut times: Vec<Num> = keys.iter().map(|&(t, _)| I(t) * F(TICK)).collect();
        let (mut qkeys, mut root_tr, mut stretches): (Vec<Vec<[Num; 4]>>, Vec<V3>, Vec<Stretch>) = (Vec::new(), Vec::new(), Vec::new());
        let mut stretch = Stretch::new();
        for &(_, p) in keys {
            let loc = pose_local(act, p, sk, short)?;
            stretch = if short { Stretch::new() } else { pose_stretch(act, p, &stretch) };
            stretches.push(stretch.clone());
            let dy = rest_low - lowest(&pose_world(sk, &loc, &stretch), verts);
            let s1 = stretch.get(&1).copied().unwrap_or([0; 3]);
            let base1 = [sk.offset[1][0] + s1[0], sk.offset[1][1] + s1[1], sk.offset[1][2] + s1[2]];
            root_tr.push(conv([I(base1[0]) + F(0.0), I(base1[1]) + dy, I(base1[2]) + F(0.0)]));
            qkeys.push(loc.iter().map(|m| mat_to_quat(&mm(&mm(&c, m), &c))).collect());
        }
        for k in 1..qkeys.len() {
            for n in 0..qkeys[k].len() {
                let (q, pq) = (qkeys[k][n], qkeys[k - 1][n]);
                if py::sum((0..4).map(|i| q[i] * pq[i])).lt(I(0)) {
                    qkeys[k][n] = q.map(|x| -x);
                }
            }
        }
        if times.len() == 1 {
            times = vec![F(0.0), F(TICK)];
            qkeys = vec![qkeys[0].clone(), qkeys[0].clone()];
            root_tr = vec![root_tr[0], root_tr[0]];
            stretches = vec![stretches[0].clone(), stretches[0].clone()];
        }
        let tv: Vec<Vec<Num>> = times.iter().map(|&t| vec![t]).collect();
        let tacc = glb.accessor(&tv, FLOAT, "SCALAR", None, true);
        let (mut samplers, mut channels) = (Vec::new(), Vec::new());
        for (n, &node) in joint_nodes.iter().enumerate() {
            let vals: Vec<Vec<Num>> = qkeys.iter().map(|qk| qk[n].to_vec()).collect();
            if vals.iter().all(|v| v[0].abs().lt(F(1e-6)) && v[1].abs().lt(F(1e-6)) && v[2].abs().lt(F(1e-6))) {
                continue;
            }
            let out = glb.accessor(&vals, FLOAT, "VEC4", None, false);
            samplers.push(Json::Obj(Obj::new().with("input", tacc).with("output", out).with("interpolation", "LINEAR")));
            channels.push(Json::Obj(Obj::new().with("sampler", samplers.len() - 1).with("target", Obj::new().with("node", node).with("path", "rotation"))));
        }
        if channels.is_empty() {
            continue;
        }
        let out = glb.accessor(&vrows(&root_tr), FLOAT, "VEC3", None, false);
        samplers.push(Json::Obj(Obj::new().with("input", tacc).with("output", out).with("interpolation", "LINEAR")));
        channels.push(Json::Obj(
            Obj::new().with("sampler", samplers.len() - 1).with("target", Obj::new().with("node", joint_nodes[1]).with("path", "translation")),
        ));
        let mut moved: Vec<usize> = stretches.iter().flat_map(|st| st.iter().filter(|(k, d)| **k != 1 && d.iter().any(|&x| x != 0)).map(|(k, _)| *k)).collect();
        moved.sort();
        moved.dedup();
        for n in moved {
            if n >= joint_nodes.len() {
                continue;
            }
            let vals: Vec<V3> = stretches
                .iter()
                .map(|st| {
                    let d = st.get(&n).copied().unwrap_or([0; 3]);
                    conv(i3([sk.offset[n][0] + d[0], sk.offset[n][1] + d[1], sk.offset[n][2] + d[2]]))
                })
                .collect();
            let out = glb.accessor(&vrows(&vals), FLOAT, "VEC3", None, false);
            samplers.push(Json::Obj(Obj::new().with("input", tacc).with("output", out).with("interpolation", "LINEAR")));
            channels.push(Json::Obj(
                Obj::new().with("sampler", samplers.len() - 1).with("target", Obj::new().with("node", joint_nodes[n]).with("path", "translation")),
            ));
        }
        let name = format!("{prefix}_{idx:03}");
        glb.list("animations").push(Json::Obj(Obj::new().with("name", name).with("samplers", samplers).with("channels", channels)));
        written += 1;
    }
    Ok(written)
}

const FLOOR_TILE: i64 = 384;

/// A floor quad: corners (PS1 space), uvs, tpage, clut.
pub type Quad = ([[i64; 3]; 4], [[i64; 2]; 4], u32, u32);

/// The arena floor from stage slots 0 (tile definitions) and 1 (tile map) (`convert.floor_quads`).
pub fn floor_quads(defs: Option<&[u8]>, fmap: Option<&[u8]>) -> Vec<Quad> {
    let (Some(defs), Some(fmap)) = (defs, fmap) else { return Vec::new() };
    if defs.is_empty() || fmap.is_empty() || defs.len() < 16 || fmap.len() < 8 {
        return Vec::new();
    }
    let count = u16::from_le_bytes([defs[4], defs[5]]) as usize;
    let (w, h, tw, th) = (fmap[4] as i64, fmap[5] as i64, fmap[6] as i64, fmap[7] as i64);
    let ntiles = (defs.len() - 8) / 8;
    let mut quads = Vec::new();
    for row in 0..h {
        for col in 0..w {
            let o = 8 + 2 * (row * w + col) as usize;
            if o + 2 > fmap.len() {
                continue;
            }
            let tid = u16::from_le_bytes([fmap[o], fmap[o + 1]]) as usize;
            if tid == 0 || tid >= count || tid >= ntiles {
                continue;
            }
            let t = &defs[8 + 8 * tid..16 + 8 * tid];
            let (u, v) = (t[0] as i64, t[1] as i64);
            let clut = u16::from_le_bytes([t[2], t[3]]) as u32;
            let flags = u16::from_le_bytes([t[4], t[5]]);
            let tpage = u16::from_le_bytes([t[6], t[7]]) as u32;
            let x0 = (col - (w >> 1)) * FLOOR_TILE;
            let z0 = -(row - (h >> 1)) * FLOOR_TILE;
            let (x1, z1) = (x0 + FLOOR_TILE, z0 - FLOOR_TILE);
            let (mut u0, mut u1, mut v0, mut v1) = (u, u + tw - 1, v, v + th - 1);
            if flags & 1 != 0 {
                (v0, v1) = (v1, v0);
            }
            if flags & 2 != 0 {
                (u0, u1) = (u1, u0);
            }
            quads.push(([[x0, 0, z0], [x1, 0, z0], [x0, 0, z1], [x1, 0, z1]], [[u0, v0], [u1, v0], [u0, v1], [u1, v1]], tpage, clut));
        }
    }
    quads
}

pub fn page_loaded(vram: &Vram, tpage: u32) -> bool {
    let (px, py) = ((tpage as usize & 0xF) * 64, ((tpage as usize >> 4) & 1) * 256);
    vram.touched[(py + 128) * crate::vram::W + px + 32]
}

/// `add_floor`: the floor quads, unlit, facing up.
pub fn add_floor(glb: &mut Glb, mats: &mut Materials, mb: &mut dyn TriSink, quads: &[Quad]) {
    let up = conv_dir([I(0), I(-1), I(0)]);
    for (corners, uvs, tpage, clut) in quads {
        if !page_loaded(mats.vram, *tpage) {
            continue;
        }
        let mat = mats.get_tp(glb, *tpage, *clut, true, true);
        let cs: Vec<Corner> = corners
            .iter()
            .zip(uvs)
            .map(|(p, uv)| Corner {
                pos: conv(i3(*p)),
                nrm: Some(up),
                uv: Some([(I(uv[0]) + F(0.5)) / F(256.0), (I(uv[1]) + F(0.5)) / F(256.0)]),
                col: None,
                joint: None,
            })
            .collect();
        for [a, b, c] in [[0, 1, 2], [1, 3, 2]] {
            let (pa, pb, pc) = (conv(i3(corners[a])), conv(i3(corners[b])), conv(i3(corners[c])));
            let n = cross(sub(pb, pa), sub(pc, pa));
            if n[1].gt(I(0)) {
                mb.tri(mat, [&cs[a], &cs[b], &cs[c]]);
            } else {
                mb.tri(mat, [&cs[a], &cs[c], &cs[b]]);
            }
        }
    }
}

/// The winding sign of a static model: its lit polygons', else the stages' common one.
pub fn static_winding(h: &Hmd, stage_winding: i64) -> Result<i64, String> {
    let geoms = hmd_geometry(h, None)?;
    Ok(if geoms.iter().any(|g| g[0].nrm.is_some()) { winding_sign(&geoms) } else { stage_winding })
}

/// `export_static`: an arena (floor, ring walls, parts, the hidden wall debris) or the boot logo. Returns the
/// files (`<name>.glb` first, then the debris pieces) and the manifest info.
pub fn export_static(
    hmds: &[(usize, &[u8])],
    vram: &Vram,
    name: &str,
    floor: &[Quad],
    stage_winding: i64,
    colors: Colors,
) -> Result<(Vec<(String, Vec<u8>)>, Obj), String> {
    let mut glb = Glb::new();
    let mut mats = Materials::new(vram, name);
    let mut nodes = Vec::new();
    let mut files = Vec::new();
    let st = name.starts_with("ST");
    if !floor.is_empty() {
        let mut mb = MeshBuilder::default();
        add_floor(&mut glb, &mut mats, &mut mb, floor);
        let n = format!("{name}_floor");
        let m = mb.build(&mut glb, &n, false);
        nodes.push(glb.node(Obj::new().with("name", n).with("mesh", m)));
    }
    for &(idx, data) in hmds {
        let h = Hmd::parse(data)?;
        let wsign = static_winding(&h, stage_winding)?;
        if idx == 3 && st {
            // slot 3: the debris pieces of a broken wall (one coordinate each, all at the origin), each also in its
            // own file <name>_debris<k>.glb
            files.extend(debris_files(&mut glb, &mut mats, &h, vram, name, wsign, colors, "")?);
        }
        let mut mb = MeshBuilder::default();
        let mut walls: Vec<MeshBuilder> = (0..4).map(|_| MeshBuilder::default()).collect();
        let has_walls = idx == 2 && st;
        {
            let mut sinks: Vec<&mut dyn TriSink> = vec![&mut mb];
            for w in walls.iter_mut() {
                sinks.push(w);
            }
            add_hmd(
                &mut glb,
                &mut mats,
                &h,
                wsign,
                None,
                None,
                colors,
                &mut |p| Some(if has_walls && p.bone < 4 { 1 + pyi(&[0, 1, 2, 3], p.bone).to_owned() } else { 0 }),
                &mut sinks,
            )?;
        }
        if has_walls {
            for (k, w) in walls.iter().enumerate() {
                if !w.is_empty() {
                    let n = format!("{name}_wall{k}");
                    let m = w.build(&mut glb, &n, false);
                    nodes.push(glb.node(Obj::new().with("name", n).with("mesh", m)));
                }
            }
        }
        let mesh = mb.build(&mut glb, &format!("{name}_part{idx:02}"), false);
        if idx == 3 && st {
            nodes.push(
                glb.node(
                    Obj::new()
                        .with("name", format!("{name}_wall_debris"))
                        .with("mesh", mesh)
                        .with("extras", Obj::new().with("hiddenByDefault", true).with("toggle", "debris")),
                ),
            );
        } else {
            nodes.push(glb.node(Obj::new().with("name", format!("{name}_part{idx:02}")).with("mesh", mesh)));
        }
    }
    let nparts = nodes.len();
    let root = glb.node(Obj::new().with("name", name).with("children", nodes));
    glb.set_scene_nodes(vec![root]);
    files.insert(0, (format!("{name}.glb"), glb.save()));
    Ok((files, Obj::new().with("parts", nparts).with("missing_textures", mats.missing.len())))
}

/// The debris pieces of slot 3 in their own files `<name>_debris<k><suffix>.glb`; `glb` / `mats` get the
/// materials made on the way (the first `add_hmd` pass).
pub fn debris_files(
    glb: &mut Glb,
    mats: &mut Materials,
    h: &Hmd,
    vram: &Vram,
    name: &str,
    wsign: i64,
    colors: Colors,
    suffix: &str,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut pieces: Vec<i64> = Vec::new();
    add_hmd(
        glb,
        mats,
        h,
        wsign,
        None,
        None,
        colors,
        &mut |p| {
            if !pieces.contains(&p.bone) {
                pieces.push(p.bone);
            }
            None
        },
        &mut [],
    )?;
    pieces.sort();
    let mut out = Vec::new();
    for k in pieces {
        let mut pg = Glb::new();
        let mut pm = Materials::new(vram, name);
        let mut pb = MeshBuilder::default();
        add_hmd(&mut pg, &mut pm, h, wsign, None, None, colors, &mut |p| if p.bone == k { Some(0) } else { None }, &mut [&mut pb])?;
        let n = format!("{name}_debris{k}");
        let m = pb.build(&mut pg, &n, false);
        let pn = pg.node(Obj::new().with("name", n.clone()).with("mesh", m));
        pg.set_scene_nodes(vec![pn]);
        out.push((format!("{n}{suffix}.glb"), pg.save()));
    }
    Ok(out)
}

/// Unused-sink helper for routes that drop polygons.
pub fn drop_sink() -> Drop {
    Drop
}
