//! glTF 2.0 binary (.glb) files in a fixed layout: the same JSON (key order, float `repr`), the
//! same buffer layout, so the files are byte-identical.

use crate::json::{Json, Obj};
use crate::py::{Num, max2, min2};
use std::collections::HashMap;

pub const FLOAT: u32 = 5126;
pub const UINT: u32 = 5125;
pub const USHORT: u32 = 5123;
pub const ARRAY_BUFFER: u32 = 34962;
pub const ELEMENT_ARRAY_BUFFER: u32 = 34963;

pub struct Glb {
    pub g: Obj,
    pub bin: Vec<u8>,
}

impl Default for Glb {
    fn default() -> Self {
        Self::new()
    }
}

impl Glb {
    pub fn new() -> Glb {
        let sampler = Obj::new().with("magFilter", 9728).with("minFilter", 9728).with("wrapS", 33071).with("wrapT", 33071);
        let g = Obj::new()
            .with("asset", Obj::new().with("version", "2.0").with("generator", "br2_tools")) // the generator name of the first version: kept, so the files stay the same
            .with("scene", 0)
            .with("scenes", vec![Json::Obj(Obj::new().with("nodes", Vec::<Json>::new()))])
            .with("nodes", Vec::<Json>::new())
            .with("meshes", Vec::<Json>::new())
            .with("materials", Vec::<Json>::new())
            .with("accessors", Vec::<Json>::new())
            .with("bufferViews", Vec::<Json>::new())
            .with("buffers", Vec::<Json>::new())
            .with("textures", Vec::<Json>::new())
            .with("images", Vec::<Json>::new())
            .with("samplers", vec![Json::Obj(sampler)]);
        Glb { g, bin: Vec::new() }
    }

    pub fn list(&mut self, k: &str) -> &mut Vec<Json> {
        self.g.entry(k, Vec::<Json>::new()).arr()
    }

    fn view(&mut self, data: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = Obj::new().with("buffer", 0).with("byteOffset", self.bin.len()).with("byteLength", data.len());
        if let Some(t) = target {
            v.set("target", t);
        }
        self.bin.extend_from_slice(data);
        let l = self.list("bufferViews");
        l.push(Json::Obj(v));
        l.len() - 1
    }

    /// `GLB.accessor`: `values` = one row of `comps` numbers per element.
    pub fn accessor(&mut self, values: &[Vec<Num>], ctype: u32, atype: &str, target: Option<u32>, minmax: bool) -> usize {
        let comps = match atype {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            "MAT4" => 16,
            _ => panic!("accessor type {atype}"),
        };
        let size = match ctype {
            FLOAT | UINT => 4,
            USHORT => 2,
            _ => 1,
        };
        let put = |out: &mut Vec<u8>, x: Num| match ctype {
            FLOAT => out.extend_from_slice(&(x.f() as f32).to_le_bytes()),
            UINT => out.extend_from_slice(&(int(x) as u32).to_le_bytes()),
            USHORT => out.extend_from_slice(&(int(x) as u16).to_le_bytes()),
            _ => out.push(int(x) as u8),
        };
        let mut data = Vec::with_capacity(values.len() * comps * size);
        let bv;
        if (ctype == USHORT || ctype == 5121) && !(comps * size).is_multiple_of(4) && target == Some(ARRAY_BUFFER) {
            let stride = (comps * size + 3) & !3;
            for v in values {
                for &x in v {
                    put(&mut data, x);
                }
                data.resize(data.len() + stride - comps * size, 0);
            }
            bv = self.view(&data, target);
            self.list("bufferViews")[bv].obj().set("byteStride", stride);
        } else {
            for v in values {
                for &x in v {
                    put(&mut data, x);
                }
            }
            bv = self.view(&data, target);
        }
        let mut a = Obj::new().with("bufferView", bv).with("componentType", ctype).with("count", values.len()).with("type", atype);
        if minmax {
            let mut mn = values[0].clone();
            let mut mx = values[0].clone();
            for v in &values[1..] {
                for i in 0..comps {
                    mn[i] = min2(mn[i], v[i]);
                    mx[i] = max2(mx[i], v[i]);
                }
            }
            a.set("min", mn);
            a.set("max", mx);
        }
        let l = self.list("accessors");
        l.push(Json::Obj(a));
        l.len() - 1
    }

    pub fn image_png(&mut self, png: &[u8], name: &str) -> usize {
        let bv = self.view(png, None);
        self.list("images").push(Json::Obj(Obj::new().with("bufferView", bv).with("mimeType", "image/png").with("name", name)));
        let src = self.list("images").len() - 1;
        let l = self.list("textures");
        l.push(Json::Obj(Obj::new().with("source", src).with("sampler", 0)));
        l.len() - 1
    }

    /// `GLB.material(name, texture)` with the defaults (single-sided, alpha mask when textured).
    pub fn material(&mut self, name: &str, texture: Option<usize>) -> usize {
        let mut pbr = Obj::new().with("metallicFactor", 0.0).with("roughnessFactor", 1.0);
        let mut m = Obj::new().with("name", name);
        if let Some(t) = texture {
            pbr.set("baseColorTexture", Obj::new().with("index", t));
        }
        m.set("pbrMetallicRoughness", pbr);
        m.set("doubleSided", false);
        if texture.is_some() {
            m.set("alphaMode", "MASK");
            m.set("alphaCutoff", 0.5);
        }
        let l = self.list("materials");
        l.push(Json::Obj(m));
        l.len() - 1
    }

    pub fn material_mut(&mut self, i: usize) -> &mut Obj {
        self.list("materials")[i].obj()
    }

    /// `GLB.node(**kw)`: the keys in the given order.
    pub fn node(&mut self, kw: Obj) -> usize {
        let l = self.list("nodes");
        l.push(Json::Obj(kw));
        l.len() - 1
    }

    pub fn node_mut(&mut self, i: usize) -> &mut Obj {
        self.list("nodes")[i].obj()
    }

    pub fn set_scene_nodes(&mut self, nodes: Vec<usize>) {
        self.list("scenes")[0].obj().set("nodes", nodes);
    }

    /// `GLB.save`: the file's bytes.
    pub fn save(mut self) -> Vec<u8> {
        for k in ["textures", "images", "materials", "meshes", "skins", "animations"] {
            if self.g.get(k).and_then(Json::as_arr).is_some_and(|a| a.is_empty()) {
                self.g.remove(k);
            }
        }
        if !self.g.contains("textures") {
            self.g.remove("samplers");
        }
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let bl = self.bin.len();
        self.g.set("buffers", vec![Json::Obj(Obj::new().with("byteLength", bl))]);
        let mut js = Json::Obj(self.g).compact().into_bytes();
        while !js.len().is_multiple_of(4) {
            js.push(b' ');
        }
        let total = 12 + 8 + js.len() + 8 + self.bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(js.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        out.extend_from_slice(&js);
        out.extend_from_slice(&(self.bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        out.extend_from_slice(&self.bin);
        out
    }
}

fn int(x: Num) -> i64 {
    match x {
        Num::I(i) => i,
        Num::F(f) => f as i64,
    }
}

/// One triangle corner: position, normal, uv, colour, joint (`MeshBuilder.tri`'s tuple).
#[derive(Clone, Debug)]
pub struct Corner {
    pub pos: [Num; 3],
    pub nrm: Option<[Num; 3]>,
    pub uv: Option<[Num; 2]>,
    pub col: Option<[Num; 3]>,
    pub joint: Option<i64>,
}

/// The dedupe key: Python's tuple equality (-0.0 == 0.0 == 0).
type Key = ([u64; 3], Option<[u64; 3]>, Option<[u64; 2]>, Option<[u64; 3]>, Option<i64>);

fn key(c: &Corner) -> Key {
    (c.pos.map(Num::key), c.nrm.map(|n| n.map(Num::key)), c.uv.map(|n| n.map(Num::key)), c.col.map(|n| n.map(Num::key)), c.joint)
}

#[derive(Default)]
pub struct Group {
    pub v: Vec<Corner>,
    map: HashMap<Key, usize>,
    pub i: Vec<usize>,
}

impl Group {
    fn add(&mut self, c: &Corner) {
        let k = key(c);
        let idx = *self.map.entry(k).or_insert_with(|| {
            self.v.push(c.clone());
            self.v.len() - 1
        });
        self.i.push(idx);
    }
}

/// Triangles grouped by material (in the order the materials first appear), vertices deduped.
#[derive(Default)]
pub struct MeshBuilder {
    pub groups: Vec<(i64, Group)>,
}

/// Where `add_hmd` puts triangles: `MeshBuilder` or the ordered mesh of stage_ot.
pub trait TriSink {
    fn tri(&mut self, mat: i64, corners: [&Corner; 3]);
}

impl TriSink for MeshBuilder {
    fn tri(&mut self, mat: i64, corners: [&Corner; 3]) {
        let gi = match self.groups.iter().position(|g| g.0 == mat) {
            Some(i) => i,
            None => {
                self.groups.push((mat, Group::default()));
                self.groups.len() - 1
            }
        };
        let g = &mut self.groups[gi].1;
        for c in corners {
            g.add(c);
        }
    }
}

/// The attributes of one primitive (`MeshBuilder.build`'s accessors).
fn prim_attrs(glb: &mut Glb, vs: &[Corner], skinned: bool) -> Obj {
    let pos: Vec<Vec<Num>> = vs.iter().map(|v| v.pos.to_vec()).collect();
    let mut attrs = Obj::new().with("POSITION", glb.accessor(&pos, FLOAT, "VEC3", Some(ARRAY_BUFFER), true));
    if vs.iter().all(|v| v.nrm.is_some()) {
        let n: Vec<Vec<Num>> = vs.iter().map(|v| v.nrm.unwrap().to_vec()).collect();
        attrs.set("NORMAL", glb.accessor(&n, FLOAT, "VEC3", Some(ARRAY_BUFFER), false));
    }
    if vs.iter().all(|v| v.uv.is_some()) {
        let n: Vec<Vec<Num>> = vs.iter().map(|v| v.uv.unwrap().to_vec()).collect();
        attrs.set("TEXCOORD_0", glb.accessor(&n, FLOAT, "VEC2", Some(ARRAY_BUFFER), false));
    }
    if vs.iter().any(|v| v.col.is_some()) {
        let n: Vec<Vec<Num>> = vs.iter().map(|v| v.col.map(|c| c.to_vec()).unwrap_or_else(|| vec![Num::F(1.0); 3])).collect();
        attrs.set("COLOR_0", glb.accessor(&n, FLOAT, "VEC3", Some(ARRAY_BUFFER), false));
    }
    if skinned {
        let j: Vec<Vec<Num>> = vs.iter().map(|v| vec![Num::I(v.joint.unwrap_or(0)), Num::I(0), Num::I(0), Num::I(0)]).collect();
        attrs.set("JOINTS_0", glb.accessor(&j, USHORT, "VEC4", Some(ARRAY_BUFFER), false));
        let w: Vec<Vec<Num>> = vs.iter().map(|_| vec![Num::F(1.0), Num::F(0.0), Num::F(0.0), Num::F(0.0)]).collect();
        attrs.set("WEIGHTS_0", glb.accessor(&w, FLOAT, "VEC4", Some(ARRAY_BUFFER), false));
    }
    attrs
}

fn primitive(glb: &mut Glb, mat: i64, vs: &[Corner], ind: &[usize], skinned: bool) -> Json {
    let attrs = prim_attrs(glb, vs, skinned);
    let iv: Vec<Vec<Num>> = ind.iter().map(|&i| vec![Num::I(i as i64)]).collect();
    let it = if vs.len() > 65535 { UINT } else { USHORT };
    let acc = glb.accessor(&iv, it, "SCALAR", Some(ELEMENT_ARRAY_BUFFER), false);
    Json::Obj(Obj::new().with("attributes", attrs).with("indices", acc).with("material", mat).with("mode", 4))
}

impl MeshBuilder {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub fn build(&self, glb: &mut Glb, name: &str, skinned: bool) -> usize {
        let mut prims = Vec::new();
        for (mat, g) in &self.groups {
            if g.v.is_empty() {
                continue;
            }
            prims.push(primitive(glb, *mat, &g.v, &g.i, skinned));
        }
        let l = glb.list("meshes");
        l.push(Json::Obj(Obj::new().with("name", name).with("primitives", prims)));
        l.len() - 1
    }
}

/// `stage_ot.OrderedMesh`: triangles keep their order; consecutive ones of a material share a primitive.
#[derive(Default)]
pub struct OrderedMesh {
    pub runs: Vec<(i64, Group)>,
}

impl TriSink for OrderedMesh {
    fn tri(&mut self, mat: i64, corners: [&Corner; 3]) {
        if self.runs.last().is_none_or(|r| r.0 != mat) {
            self.runs.push((mat, Group::default()));
        }
        let g = &mut self.runs.last_mut().unwrap().1;
        for c in corners {
            g.add(c);
        }
    }
}

impl OrderedMesh {
    pub fn build(&self, glb: &mut Glb, name: &str) -> usize {
        let mut prims = Vec::new();
        for (mat, g) in &self.runs {
            prims.push(primitive(glb, *mat, &g.v, &g.i, false));
        }
        let l = glb.list("meshes");
        l.push(Json::Obj(Obj::new().with("name", name).with("primitives", prims)));
        l.len() - 1
    }
}

/// A sink that drops everything (throwaway mesh arguments).
pub struct Drop;

impl TriSink for Drop {
    fn tri(&mut self, _mat: i64, _corners: [&Corner; 3]) {}
}
