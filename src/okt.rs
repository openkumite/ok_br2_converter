//! The table file `engine/tables.okt`: named, typed tables cut out of the game's executable, so that a reader of the
//! data needs no executable (format: `docs/tables.md`).
//!
//! ```text
//! "BR2TABLE" | u32 version = 1 | u32 count |
//! count × { u8 name_len, name (UTF-8), u8 elem, u16 rec_size, u32 count, u32 origin, u32 byte_len, bytes, pad to 4 }
//! ```
//!
//! All numbers little-endian. `elem`: 0 blob (count = byte_len), 1 u8, 2 i16, 3 u16, 4 u32 (e.g. count 1: a scalar),
//! 5 record (`rec_size` bytes each). `origin` is the address the table's first byte has in the address space its
//! own links use (a table holding pointers to other tables: they point into those tables' origins); `byte_len` =
//! count × the element's size. The bytes are the executable's, verbatim (the text table `text.strings`: its C strings,
//! trimmed). Padding is to a multiple of 4 from the file's start.

/// The file's magic and version.
pub const MAGIC: &[u8; 8] = b"BR2TABLE";
pub const VERSION: u32 = 1;

/// A table's element type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elem {
    Blob,
    U8,
    I16,
    U16,
    U32,
    /// records of this many bytes
    Rec(u16),
}

impl Elem {
    fn code(self) -> u8 {
        match self {
            Elem::Blob => 0,
            Elem::U8 => 1,
            Elem::I16 => 2,
            Elem::U16 => 3,
            Elem::U32 => 4,
            Elem::Rec(_) => 5,
        }
    }

    /// The size of one element in bytes.
    pub fn size(self) -> usize {
        match self {
            Elem::Blob | Elem::U8 => 1,
            Elem::I16 | Elem::U16 => 2,
            Elem::U32 => 4,
            Elem::Rec(n) => n as usize,
        }
    }
}

/// One table: its name, element type, count and origin, and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    pub elem: Elem,
    pub count: u32,
    pub origin: u32,
    pub bytes: Vec<u8>,
}

/// Writes the tables in their order (names must be unique, and each table's bytes must be count × its element's
/// size: both checked, an error names the table).
pub fn write(tables: &[Table]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(tables.len() as u32).to_le_bytes());
    for (k, t) in tables.iter().enumerate() {
        if tables[..k].iter().any(|u| u.name == t.name) {
            return Err(format!("tables.okt: {} twice", t.name));
        }
        if t.name.is_empty() || t.name.len() > 255 {
            return Err(format!("tables.okt: a name of {} bytes", t.name.len()));
        }
        if t.elem == Elem::Rec(0) {
            return Err(format!("tables.okt: {}: records of 0 bytes", t.name));
        }
        if (t.count as usize).checked_mul(t.elem.size()) != Some(t.bytes.len()) {
            return Err(format!("tables.okt: {}: {} bytes for {} × {}", t.name, t.bytes.len(), t.count, t.elem.size()));
        }
        out.push(t.name.len() as u8);
        out.extend_from_slice(t.name.as_bytes());
        out.push(t.elem.code());
        out.extend_from_slice(&(if let Elem::Rec(n) = t.elem { n } else { 0 }).to_le_bytes());
        out.extend_from_slice(&t.count.to_le_bytes());
        out.extend_from_slice(&t.origin.to_le_bytes());
        out.extend_from_slice(&(t.bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&t.bytes);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }
    Ok(out)
}

/// Reads a file `write` wrote (the converter's tests): Err on anything `write` would not
/// write (a name twice, a byte length other than count × the element's size, records of 0 bytes, a file cut short).
pub fn read(b: &[u8]) -> Result<Vec<Table>, String> {
    let get = |o: usize, n: usize| o.checked_add(n).and_then(|e| b.get(o..e)).ok_or("tables.okt: cut short");
    let u32_at = |o: usize| get(o, 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()));
    if b.get(..8) != Some(&MAGIC[..]) || u32_at(8)? != VERSION {
        return Err("tables.okt: not a version 1 table file".into());
    }
    let n = u32_at(12)? as usize;
    let mut o = 16;
    let mut v = Vec::new();
    for _ in 0..n {
        let len = get(o, 1)?[0] as usize;
        let name = String::from_utf8(get(o + 1, len)?.to_vec()).map_err(|e| e.to_string())?;
        if v.iter().any(|t: &Table| t.name == name) {
            return Err(format!("tables.okt: {name} twice"));
        }
        o += 1 + len;
        let code = get(o, 1)?[0];
        let rec = u16::from_le_bytes(get(o + 1, 2)?.try_into().unwrap());
        let elem = match code {
            0 => Elem::Blob,
            1 => Elem::U8,
            2 => Elem::I16,
            3 => Elem::U16,
            4 => Elem::U32,
            5 => Elem::Rec(rec),
            c => return Err(format!("tables.okt: {name}: element type {c}")),
        };
        let (count, origin, byte_len) = (u32_at(o + 3)?, u32_at(o + 7)?, u32_at(o + 11)? as usize);
        if elem == Elem::Rec(0) {
            return Err(format!("tables.okt: {name}: records of 0 bytes"));
        }
        if (count as usize).checked_mul(elem.size()) != Some(byte_len) {
            return Err(format!("tables.okt: {name}: {byte_len} bytes for {count} × {}", elem.size()));
        }
        o += 15;
        let bytes = get(o, byte_len)?.to_vec();
        o += byte_len;
        o = o.div_ceil(4) * 4;
        v.push(Table { name, elem, count, origin, bytes });
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_checks() {
        let t = vec![
            Table { name: "a.b".into(), elem: Elem::I16, count: 2, origin: 0x10, bytes: vec![1, 0, 0xFF, 0xFF] },
            Table { name: "c".into(), elem: Elem::Rec(3), count: 1, origin: 7, bytes: vec![9, 8, 7] },
            Table { name: "d".into(), elem: Elem::U32, count: 1, origin: 0, bytes: vec![1, 2, 3, 4] },
        ];
        let b = write(&t).unwrap();
        assert_eq!(&b[..8], MAGIC);
        assert_eq!(b.len() % 4, 0);
        assert_eq!(read(&b).unwrap(), t);
        let mut twice = t.clone();
        twice[1].name = "a.b".into();
        assert!(write(&twice).unwrap_err().contains("twice"));
        let mut short = t.clone();
        short[0].bytes.pop();
        assert!(write(&short).unwrap_err().contains("a.b"));
        assert!(read(&b[..b.len() - 5]).is_err());
        // a byte length that is not count × size, and a huge one, are refused
        let mut bad = b.clone();
        bad[8 + 4 + 4 + 1 + 3 + 1 + 2 + 4 + 4] = 3;
        assert!(read(&bad).unwrap_err().contains("a.b"));
        bad[8 + 4 + 4 + 1 + 3 + 1 + 2 + 4 + 4..][..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&bad).is_err());
        // records of 0 bytes: refused with their own message, written or read
        let mut zero = t.clone();
        (zero[1].elem, zero[1].bytes) = (Elem::Rec(0), Vec::new());
        assert!(write(&zero).unwrap_err().contains("c: records of 0 bytes"));
        let mut z = b.clone();
        // the second table's element code: after the first (23 bytes, padded to 24), its name's length and name
        let c = 8 + 4 + 4 + 24 + 1 + 1;
        assert_eq!(z[c], 5);
        z[c + 1..c + 3].copy_from_slice(&0u16.to_le_bytes());
        assert!(read(&z).unwrap_err().contains("c: records of 0 bytes"));
        // a name twice (the second table's one-byte name "c" made the third's "d"): refused when read as when written
        let mut dup = b.clone();
        let at = 8 + 4 + 4 + 24 + 1;
        assert_eq!(dup[at], b'c');
        dup[at] = b'd';
        assert!(read(&dup).unwrap_err().contains("d twice"));
    }
}
