//! zlib 1.2.12's `compress2(level 9)`, so the PNGs come out byte for byte the same on every target, as zlib's level 9
//! writes them (deflate.c `deflate_slow` with the level-9 configuration, trees.c's Huffman trees and block choice;
//! window 32 KiB, memLevel 8).

const WSIZE: usize = 1 << 15;
const WMASK: usize = WSIZE - 1;
const WINDOW_SIZE: usize = 2 * WSIZE;
const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: u32 = HASH_SIZE as u32 - 1;
const HASH_SHIFT: u32 = HASH_BITS.div_ceil(3);
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const MIN_LOOKAHEAD: usize = MAX_MATCH + MIN_MATCH + 1;
const MAX_DIST: usize = WSIZE - MIN_LOOKAHEAD;
const TOO_FAR: usize = 4096;
const LIT_BUFSIZE: usize = 1 << (8 + 6);
const SYM_END: usize = (LIT_BUFSIZE - 1) * 3;
// level 9: good_length, max_lazy, nice_length, max_chain
const GOOD_MATCH: usize = 32;
const MAX_LAZY: usize = 258;
const NICE_MATCH: usize = 258;
const MAX_CHAIN: u32 = 4096;

const LENGTH_CODES: usize = 29;
const LITERALS: usize = 256;
const L_CODES: usize = LITERALS + 1 + LENGTH_CODES;
const D_CODES: usize = 30;
const BL_CODES: usize = 19;
const HEAP_SIZE: usize = 2 * L_CODES + 1;
const MAX_BITS: usize = 15;
const MAX_BL_BITS: usize = 7;
const END_BLOCK: usize = 256;
const REP_3_6: usize = 16;
const REPZ_3_10: usize = 17;
const REPZ_11_138: usize = 18;

const EXTRA_LBITS: [u32; LENGTH_CODES] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const EXTRA_DBITS: [u32; D_CODES] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const EXTRA_BLBITS: [u32; BL_CODES] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];
const BL_ORDER: [usize; BL_CODES] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// trees.c's static tables (tr_static_init).
struct Tables {
    length_code: [u8; 256],
    dist_code: [u8; 512],
    base_length: [u32; LENGTH_CODES],
    base_dist: [u32; D_CODES],
    static_ltree: [(u16, u16); L_CODES + 2],
    static_dtree: [(u16, u16); D_CODES],
}

fn bi_reverse(mut code: u32, mut len: u32) -> u32 {
    let mut res = 0;
    loop {
        res |= code & 1;
        code >>= 1;
        res <<= 1;
        len -= 1;
        if len == 0 {
            break;
        }
    }
    res >> 1
}

fn tables() -> &'static Tables {
    static T: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = Tables {
            length_code: [0; 256],
            dist_code: [0; 512],
            base_length: [0; LENGTH_CODES],
            base_dist: [0; D_CODES],
            static_ltree: [(0, 0); L_CODES + 2],
            static_dtree: [(0, 0); D_CODES],
        };
        let mut length = 0u32;
        for code in 0..LENGTH_CODES - 1 {
            t.base_length[code] = length;
            for _ in 0..(1 << EXTRA_LBITS[code]) {
                t.length_code[length as usize] = code as u8;
                length += 1;
            }
        }
        t.length_code[length as usize - 1] = (LENGTH_CODES - 1) as u8;
        let mut dist = 0u32;
        for code in 0..16 {
            t.base_dist[code] = dist;
            for _ in 0..(1 << EXTRA_DBITS[code]) {
                t.dist_code[dist as usize] = code as u8;
                dist += 1;
            }
        }
        dist >>= 7;
        for code in 16..D_CODES {
            t.base_dist[code] = dist << 7;
            for _ in 0..(1 << (EXTRA_DBITS[code] - 7)) {
                t.dist_code[256 + dist as usize] = code as u8;
                dist += 1;
            }
        }
        let mut bl_count = [0u16; MAX_BITS + 1];
        let mut lens = [0u16; L_CODES + 2];
        for (n, l) in lens.iter_mut().enumerate() {
            *l = match n {
                0..=143 => 8,
                144..=255 => 9,
                256..=279 => 7,
                _ => 8,
            };
            bl_count[*l as usize] += 1;
        }
        let codes = gen_codes_raw(&lens, L_CODES + 1, &bl_count);
        for n in 0..L_CODES + 2 {
            t.static_ltree[n] = (codes[n], lens[n]);
        }
        for n in 0..D_CODES {
            t.static_dtree[n] = (bi_reverse(n as u32, 5) as u16, 5);
        }
        t
    })
}

fn gen_codes_raw(lens: &[u16], max_code: usize, bl_count: &[u16; MAX_BITS + 1]) -> Vec<u16> {
    let mut next_code = [0u16; MAX_BITS + 1];
    let mut code: u32 = 0;
    for bits in 1..=MAX_BITS {
        code = (code + bl_count[bits - 1] as u32) << 1;
        next_code[bits] = code as u16;
    }
    let mut out = vec![0u16; lens.len()];
    for n in 0..=max_code {
        let len = lens[n] as usize;
        if len == 0 {
            continue;
        }
        out[n] = bi_reverse(next_code[len] as u32, len as u32) as u16;
        next_code[len] += 1;
    }
    out
}

fn d_code(t: &Tables, dist: usize) -> usize {
    if dist < 256 { t.dist_code[dist] as usize } else { t.dist_code[256 + (dist >> 7)] as usize }
}

/// One dynamic tree (ct_data: `fc` = freq / code, `dl` = dad / len, as the C unions).
struct Tree {
    fc: Vec<u16>,
    dl: Vec<u16>,
    max_code: i32,
}

#[derive(Clone, Copy)]
enum Kind {
    L,
    D,
    Bl,
}

struct BitOut {
    out: Vec<u8>,
    buf: u64,
    n: u32,
}

impl BitOut {
    fn send(&mut self, value: u32, len: u32) {
        self.buf |= (value as u64) << self.n;
        self.n += len;
        while self.n >= 8 {
            self.out.push(self.buf as u8);
            self.buf >>= 8;
            self.n -= 8;
        }
    }
    fn windup(&mut self) {
        if self.n > 0 {
            self.out.push(self.buf as u8);
        }
        self.buf = 0;
        self.n = 0;
    }
}

struct State<'a> {
    input: &'a [u8],
    in_pos: usize,
    window: Box<[u8; WINDOW_SIZE]>,
    prev: Box<[u16; WSIZE]>,
    head: Box<[u16; HASH_SIZE]>,
    ins_h: u32,
    block_start: i64,
    match_length: usize,
    prev_match: usize,
    match_available: bool,
    strstart: usize,
    match_start: usize,
    lookahead: usize,
    prev_length: usize,
    insert: usize,
    // trees
    ltree: Tree,
    dtree: Tree,
    bltree: Tree,
    bl_count: [u16; MAX_BITS + 1],
    heap: [i32; HEAP_SIZE],
    heap_len: usize,
    heap_max: usize,
    depth: [u8; HEAP_SIZE],
    sym_buf: Vec<u8>,
    sym_next: usize,
    opt_len: u64,
    static_len: u64,
    bits: BitOut,
}

impl<'a> State<'a> {
    fn tree(&mut self, k: Kind) -> &mut Tree {
        match k {
            Kind::L => &mut self.ltree,
            Kind::D => &mut self.dtree,
            Kind::Bl => &mut self.bltree,
        }
    }

    fn update_hash(&mut self, c: u8) {
        self.ins_h = ((self.ins_h << HASH_SHIFT) ^ c as u32) & HASH_MASK;
    }

    fn insert_string(&mut self, s: usize) -> usize {
        self.update_hash(self.window[s + MIN_MATCH - 1]);
        let h = self.head[self.ins_h as usize];
        self.prev[s & WMASK] = h;
        self.head[self.ins_h as usize] = s as u16;
        h as usize
    }

    fn fill_window(&mut self) {
        loop {
            let mut more = WINDOW_SIZE - self.lookahead - self.strstart;
            if self.strstart >= WSIZE + MAX_DIST {
                let n = WSIZE - more;
                self.window.copy_within(WSIZE..WSIZE + n, 0);
                self.match_start = self.match_start.wrapping_sub(WSIZE);
                self.strstart -= WSIZE;
                self.block_start -= WSIZE as i64;
                if self.insert > self.strstart {
                    self.insert = self.strstart;
                }
                for p in self.head.iter_mut().chain(self.prev.iter_mut()) {
                    let m = *p as usize;
                    *p = if m >= WSIZE { (m - WSIZE) as u16 } else { 0 };
                }
                more += WSIZE;
            }
            if self.in_pos == self.input.len() {
                break;
            }
            let n = more.min(self.input.len() - self.in_pos);
            let dst = self.strstart + self.lookahead;
            self.window[dst..dst + n].copy_from_slice(&self.input[self.in_pos..self.in_pos + n]);
            self.in_pos += n;
            self.lookahead += n;
            if self.lookahead + self.insert >= MIN_MATCH {
                let mut s = self.strstart - self.insert;
                self.ins_h = self.window[s] as u32;
                self.update_hash(self.window[s + 1]);
                while self.insert > 0 {
                    self.update_hash(self.window[s + MIN_MATCH - 1]);
                    self.prev[s & WMASK] = self.head[self.ins_h as usize];
                    self.head[self.ins_h as usize] = s as u16;
                    s += 1;
                    self.insert -= 1;
                    if self.lookahead + self.insert < MIN_MATCH {
                        break;
                    }
                }
            }
            if !(self.lookahead < MIN_LOOKAHEAD && self.in_pos != self.input.len()) {
                break;
            }
        }
    }

    fn longest_match(&mut self, mut cur_match: usize) -> usize {
        let mut chain_length = MAX_CHAIN;
        let scan = self.strstart;
        let mut best_len = self.prev_length;
        let mut nice_match = NICE_MATCH;
        let limit = self.strstart.saturating_sub(MAX_DIST);
        // every index stays inside the window (strstart + MAX_MATCH < WINDOW_SIZE); the masks only let the
        // compiler drop the bounds checks
        const M: usize = WINDOW_SIZE - 1;
        let w: &[u8; WINDOW_SIZE] = &self.window;
        let prev: &[u16; WSIZE] = &self.prev;
        let (s0, s1) = (w[scan & M], w[(scan + 1) & M]);
        let mut scan_end1 = w[(scan + best_len - 1) & M];
        let mut scan_end = w[(scan + best_len) & M];
        if self.prev_length >= GOOD_MATCH {
            chain_length >>= 2;
        }
        if nice_match > self.lookahead {
            nice_match = self.lookahead;
        }
        loop {
            let m = cur_match;
            let skip = w[(m + best_len) & M] != scan_end || w[(m + best_len - 1) & M] != scan_end1 || w[m & M] != s0 || w[(m + 1) & M] != s1;
            if !skip {
                // bytes 0, 1 checked; byte 2 is equal by the hash; compare from 3 up to MAX_MATCH
                let len = match_len(&w[scan..scan + MAX_MATCH], &w[m..m + MAX_MATCH]);
                if len > best_len {
                    self.match_start = cur_match;
                    best_len = len;
                    if len >= nice_match {
                        break;
                    }
                    scan_end1 = w[(scan + best_len - 1) & M];
                    scan_end = w[(scan + best_len) & M];
                }
            }
            cur_match = prev[cur_match & WMASK] as usize;
            if cur_match <= limit {
                break;
            }
            chain_length -= 1;
            if chain_length == 0 {
                break;
            }
        }
        if best_len <= self.lookahead { best_len } else { self.lookahead }
    }

    fn tally_lit(&mut self, c: u8) -> bool {
        self.sym_buf[self.sym_next] = 0;
        self.sym_buf[self.sym_next + 1] = 0;
        self.sym_buf[self.sym_next + 2] = c;
        self.sym_next += 3;
        self.ltree.fc[c as usize] += 1;
        self.sym_next == SYM_END
    }

    fn tally_dist(&mut self, dist: usize, len: usize) -> bool {
        let t = tables();
        self.sym_buf[self.sym_next] = dist as u8;
        self.sym_buf[self.sym_next + 1] = (dist >> 8) as u8;
        self.sym_buf[self.sym_next + 2] = len as u8;
        self.sym_next += 3;
        let dist = dist - 1;
        self.ltree.fc[t.length_code[len] as usize + LITERALS + 1] += 1;
        self.dtree.fc[d_code(t, dist)] += 1;
        self.sym_next == SYM_END
    }

    fn init_block(&mut self) {
        for n in 0..L_CODES {
            self.ltree.fc[n] = 0;
        }
        for n in 0..D_CODES {
            self.dtree.fc[n] = 0;
        }
        for n in 0..BL_CODES {
            self.bltree.fc[n] = 0;
        }
        self.ltree.fc[END_BLOCK] = 1;
        self.opt_len = 0;
        self.static_len = 0;
        self.sym_next = 0;
    }

    fn smaller(&self, k: Kind, n: i32, m: i32) -> bool {
        let fc = match k {
            Kind::L => &self.ltree.fc,
            Kind::D => &self.dtree.fc,
            Kind::Bl => &self.bltree.fc,
        };
        let (n, m) = (n as usize, m as usize);
        fc[n] < fc[m] || (fc[n] == fc[m] && self.depth[n] <= self.depth[m])
    }

    fn pqdownheap(&mut self, k: Kind, mut i: usize) {
        let v = self.heap[i];
        let mut j = i << 1;
        while j <= self.heap_len {
            if j < self.heap_len && self.smaller(k, self.heap[j + 1], self.heap[j]) {
                j += 1;
            }
            if self.smaller(k, v, self.heap[j]) {
                break;
            }
            self.heap[i] = self.heap[j];
            i = j;
            j <<= 1;
        }
        self.heap[i] = v;
    }

    fn build_tree(&mut self, k: Kind) {
        let t = tables();
        let (elems, stree): (usize, Option<&[(u16, u16)]>) = match k {
            Kind::L => (L_CODES, Some(&t.static_ltree[..])),
            Kind::D => (D_CODES, Some(&t.static_dtree[..])),
            Kind::Bl => (BL_CODES, None),
        };
        let mut max_code: i32 = -1;
        self.heap_len = 0;
        self.heap_max = HEAP_SIZE;
        for n in 0..elems {
            if self.tree(k).fc[n] != 0 {
                self.heap_len += 1;
                self.heap[self.heap_len] = n as i32;
                max_code = n as i32;
                self.depth[n] = 0;
            } else {
                self.tree(k).dl[n] = 0;
            }
        }
        while self.heap_len < 2 {
            let node = if max_code < 2 {
                max_code += 1;
                max_code
            } else {
                0
            };
            self.heap_len += 1;
            self.heap[self.heap_len] = node;
            self.tree(k).fc[node as usize] = 1;
            self.depth[node as usize] = 0;
            self.opt_len = self.opt_len.wrapping_sub(1);
            if let Some(st) = stree {
                self.static_len = self.static_len.wrapping_sub(st[node as usize].1 as u64);
            }
        }
        self.tree(k).max_code = max_code;
        let mut n = self.heap_len / 2;
        while n >= 1 {
            self.pqdownheap(k, n);
            n -= 1;
        }
        let mut node = elems;
        loop {
            // pqremove
            let n = self.heap[1];
            self.heap[1] = self.heap[self.heap_len];
            self.heap_len -= 1;
            self.pqdownheap(k, 1);
            let m = self.heap[1];
            self.heap_max -= 1;
            self.heap[self.heap_max] = n;
            self.heap_max -= 1;
            self.heap[self.heap_max] = m;
            let (nu, mu) = (n as usize, m as usize);
            let f = self.tree(k).fc[nu].wrapping_add(self.tree(k).fc[mu]);
            self.tree(k).fc[node] = f;
            self.depth[node] = self.depth[nu].max(self.depth[mu]) + 1;
            self.tree(k).dl[nu] = node as u16;
            self.tree(k).dl[mu] = node as u16;
            self.heap[1] = node as i32;
            node += 1;
            self.pqdownheap(k, 1);
            if self.heap_len < 2 {
                break;
            }
        }
        self.heap_max -= 1;
        self.heap[self.heap_max] = self.heap[1];
        self.gen_bitlen(k);
        let max_code = self.tree(k).max_code as usize;
        let bl_count = self.bl_count;
        let tr = self.tree(k);
        let codes = gen_codes_raw(&tr.dl, max_code, &bl_count);
        for n in 0..=max_code {
            if tr.dl[n] != 0 {
                tr.fc[n] = codes[n];
            }
        }
    }

    fn gen_bitlen(&mut self, k: Kind) {
        let t = tables();
        let (stree, extra, base, max_length): (Option<&[(u16, u16)]>, &[u32], usize, u32) = match k {
            Kind::L => (Some(&t.static_ltree[..]), &EXTRA_LBITS[..], LITERALS + 1, MAX_BITS as u32),
            Kind::D => (Some(&t.static_dtree[..]), &EXTRA_DBITS[..], 0, MAX_BITS as u32),
            Kind::Bl => (None, &EXTRA_BLBITS[..], 0, MAX_BL_BITS as u32),
        };
        let max_code = self.tree(k).max_code as usize;
        self.bl_count = [0; MAX_BITS + 1];
        let root = self.heap[self.heap_max] as usize;
        self.tree(k).dl[root] = 0;
        let mut overflow = 0i32;
        let mut h = self.heap_max + 1;
        while h < HEAP_SIZE {
            let n = self.heap[h] as usize;
            let dad = self.tree(k).dl[n] as usize;
            let mut bits = self.tree(k).dl[dad] as u32 + 1;
            if bits > max_length {
                bits = max_length;
                overflow += 1;
            }
            self.tree(k).dl[n] = bits as u16;
            h += 1;
            if n > max_code {
                continue;
            }
            self.bl_count[bits as usize] += 1;
            let xbits = if n >= base { extra[n - base] } else { 0 };
            let f = self.tree(k).fc[n] as u64;
            self.opt_len = self.opt_len.wrapping_add(f * (bits + xbits) as u64);
            if let Some(st) = stree {
                self.static_len = self.static_len.wrapping_add(f * (st[n].1 as u32 + xbits) as u64);
            }
        }
        if overflow == 0 {
            return;
        }
        loop {
            let mut bits = max_length as usize - 1;
            while self.bl_count[bits] == 0 {
                bits -= 1;
            }
            self.bl_count[bits] -= 1;
            self.bl_count[bits + 1] += 2;
            self.bl_count[max_length as usize] -= 1;
            overflow -= 2;
            if overflow <= 0 {
                break;
            }
        }
        let mut h = HEAP_SIZE;
        let mut bits = max_length as usize;
        while bits != 0 {
            let mut n = self.bl_count[bits];
            while n != 0 {
                h -= 1;
                let m = self.heap[h] as usize;
                if m > max_code {
                    continue;
                }
                if self.tree(k).dl[m] as usize != bits {
                    let f = self.tree(k).fc[m] as u64;
                    let len = self.tree(k).dl[m] as u64;
                    self.opt_len = self.opt_len.wrapping_add((bits as u64).wrapping_sub(len).wrapping_mul(f));
                    self.tree(k).dl[m] = bits as u16;
                }
                n -= 1;
            }
            bits -= 1;
        }
    }

    fn scan_tree(&mut self, k: Kind) {
        let max_code = self.tree(k).max_code;
        let mut prevlen: i32 = -1;
        let mut nextlen = self.tree(k).dl[0] as i32;
        let mut count = 0;
        let (mut max_count, mut min_count) = (7, 4);
        if nextlen == 0 {
            max_count = 138;
            min_count = 3;
        }
        self.tree(k).dl[(max_code + 1) as usize] = 0xFFFF;
        for n in 0..=max_code {
            let curlen = nextlen;
            nextlen = self.tree(k).dl[(n + 1) as usize] as i32;
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                self.bltree.fc[curlen as usize] += count as u16;
            } else if curlen != 0 {
                if curlen != prevlen {
                    self.bltree.fc[curlen as usize] += 1;
                }
                self.bltree.fc[REP_3_6] += 1;
            } else if count <= 10 {
                self.bltree.fc[REPZ_3_10] += 1;
            } else {
                self.bltree.fc[REPZ_11_138] += 1;
            }
            count = 0;
            prevlen = curlen;
            if nextlen == 0 {
                max_count = 138;
                min_count = 3;
            } else if curlen == nextlen {
                max_count = 6;
                min_count = 3;
            } else {
                max_count = 7;
                min_count = 4;
            }
        }
    }

    fn send_code(&mut self, c: usize, k: Kind) {
        let (code, len) = (self.tree(k).fc[c] as u32, self.tree(k).dl[c] as u32);
        self.bits.send(code, len);
    }

    fn send_tree(&mut self, k: Kind, max_code: i32) {
        let mut prevlen: i32 = -1;
        let mut nextlen = self.tree(k).dl[0] as i32;
        let mut count = 0;
        let (mut max_count, mut min_count) = (7, 4);
        if nextlen == 0 {
            max_count = 138;
            min_count = 3;
        }
        for n in 0..=max_code {
            let curlen = nextlen;
            nextlen = self.tree(k).dl[(n + 1) as usize] as i32;
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                loop {
                    self.send_code(curlen as usize, Kind::Bl);
                    count -= 1;
                    if count == 0 {
                        break;
                    }
                }
            } else if curlen != 0 {
                if curlen != prevlen {
                    self.send_code(curlen as usize, Kind::Bl);
                    count -= 1;
                }
                self.send_code(REP_3_6, Kind::Bl);
                self.bits.send(count as u32 - 3, 2);
            } else if count <= 10 {
                self.send_code(REPZ_3_10, Kind::Bl);
                self.bits.send(count as u32 - 3, 3);
            } else {
                self.send_code(REPZ_11_138, Kind::Bl);
                self.bits.send(count as u32 - 11, 7);
            }
            count = 0;
            prevlen = curlen;
            if nextlen == 0 {
                max_count = 138;
                min_count = 3;
            } else if curlen == nextlen {
                max_count = 6;
                min_count = 3;
            } else {
                max_count = 7;
                min_count = 4;
            }
        }
    }

    fn build_bl_tree(&mut self) -> usize {
        self.scan_tree(Kind::L);
        self.scan_tree(Kind::D);
        self.build_tree(Kind::Bl);
        let mut max_blindex = BL_CODES - 1;
        while max_blindex >= 3 {
            if self.bltree.dl[BL_ORDER[max_blindex]] != 0 {
                break;
            }
            max_blindex -= 1;
        }
        self.opt_len = self.opt_len.wrapping_add(3 * (max_blindex as u64 + 1) + 5 + 5 + 4);
        max_blindex
    }

    fn compress_block(&mut self, stat: bool) {
        let t = tables();
        let mut sx = 0;
        let send = |s: &mut State, c: usize, l: bool| {
            if stat {
                let (code, len) = if l { t.static_ltree[c] } else { t.static_dtree[c] };
                s.bits.send(code as u32, len as u32);
            } else {
                s.send_code(c, if l { Kind::L } else { Kind::D });
            }
        };
        while sx < self.sym_next {
            let mut dist = self.sym_buf[sx] as usize | (self.sym_buf[sx + 1] as usize) << 8;
            let mut lc = self.sym_buf[sx + 2] as usize;
            sx += 3;
            if dist == 0 {
                send(self, lc, true);
            } else {
                let code = t.length_code[lc] as usize;
                send(self, code + LITERALS + 1, true);
                let extra = EXTRA_LBITS[code];
                if extra != 0 {
                    lc -= t.base_length[code] as usize;
                    self.bits.send(lc as u32, extra);
                }
                dist -= 1;
                let code = d_code(t, dist);
                send(self, code, false);
                let extra = EXTRA_DBITS[code];
                if extra != 0 {
                    dist -= t.base_dist[code] as usize;
                    self.bits.send(dist as u32, extra);
                }
            }
        }
        send(self, END_BLOCK, true);
    }

    fn flush_block(&mut self, last: bool) {
        let buf = if self.block_start >= 0 { Some(self.block_start as usize) } else { None };
        let stored_len = (self.strstart as i64 - self.block_start) as u64;
        self.build_tree(Kind::L);
        self.build_tree(Kind::D);
        let max_blindex = self.build_bl_tree();
        let mut opt_lenb = self.opt_len.wrapping_add(3 + 7) >> 3;
        let static_lenb = self.static_len.wrapping_add(3 + 7) >> 3;
        if static_lenb <= opt_lenb {
            opt_lenb = static_lenb;
        }
        if let Some(b) = buf.filter(|_| stored_len + 4 <= opt_lenb) {
            self.bits.send(last as u32, 3);
            self.bits.windup();
            let n = stored_len as usize;
            self.bits.out.extend_from_slice(&(n as u16).to_le_bytes());
            self.bits.out.extend_from_slice(&(!(n as u16)).to_le_bytes());
            let data = self.window[b..b + n].to_vec();
            self.bits.out.extend_from_slice(&data);
        } else if static_lenb == opt_lenb {
            self.bits.send((1 << 1) + last as u32, 3);
            self.compress_block(true);
        } else {
            self.bits.send((2 << 1) + last as u32, 3);
            let (lcodes, dcodes, blcodes) = (self.ltree.max_code + 1, self.dtree.max_code + 1, max_blindex + 1);
            self.bits.send(lcodes as u32 - 257, 5);
            self.bits.send(dcodes as u32 - 1, 5);
            self.bits.send(blcodes as u32 - 4, 4);
            for rank in 0..blcodes {
                let l = self.bltree.dl[BL_ORDER[rank]] as u32;
                self.bits.send(l, 3);
            }
            self.send_tree(Kind::L, lcodes - 1);
            self.send_tree(Kind::D, dcodes - 1);
            self.compress_block(false);
        }
        self.init_block();
        if last {
            self.bits.windup();
        }
        self.block_start = self.strstart as i64;
    }

    fn deflate_slow(&mut self) {
        loop {
            if self.lookahead < MIN_LOOKAHEAD {
                self.fill_window();
                if self.lookahead == 0 {
                    break;
                }
            }
            let mut hash_head = 0;
            if self.lookahead >= MIN_MATCH {
                hash_head = self.insert_string(self.strstart);
            }
            self.prev_length = self.match_length;
            self.prev_match = self.match_start;
            self.match_length = MIN_MATCH - 1;
            if hash_head != 0 && self.prev_length < MAX_LAZY && self.strstart - hash_head <= MAX_DIST {
                self.match_length = self.longest_match(hash_head);
                if self.match_length <= 5 && self.match_length == MIN_MATCH && self.strstart - self.match_start > TOO_FAR {
                    self.match_length = MIN_MATCH - 1;
                }
            }
            if self.prev_length >= MIN_MATCH && self.match_length <= self.prev_length {
                let max_insert = self.strstart + self.lookahead - MIN_MATCH;
                let bflush = self.tally_dist(self.strstart - 1 - self.prev_match, self.prev_length - MIN_MATCH);
                self.lookahead -= self.prev_length - 1;
                self.prev_length -= 2;
                loop {
                    self.strstart += 1;
                    if self.strstart <= max_insert {
                        self.insert_string(self.strstart);
                    }
                    self.prev_length -= 1;
                    if self.prev_length == 0 {
                        break;
                    }
                }
                self.match_available = false;
                self.match_length = MIN_MATCH - 1;
                self.strstart += 1;
                if bflush {
                    self.flush_block(false);
                }
            } else if self.match_available {
                let c = self.window[self.strstart - 1];
                if self.tally_lit(c) {
                    self.flush_block(false);
                }
                self.strstart += 1;
                self.lookahead -= 1;
            } else {
                self.match_available = true;
                self.strstart += 1;
                self.lookahead -= 1;
            }
        }
        if self.match_available {
            let c = self.window[self.strstart - 1];
            self.tally_lit(c);
            self.match_available = false;
        }
        self.insert = self.strstart.min(MIN_MATCH - 1);
        self.flush_block(true);
    }
}

/// The first index from 3 on where `a` and `b` differ, or MAX_MATCH (8 bytes at a time).
fn match_len(a: &[u8], b: &[u8]) -> usize {
    let mut i = 3;
    while i + 8 <= MAX_MATCH {
        let x = u64::from_le_bytes(a[i..i + 8].try_into().unwrap()) ^ u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        if x != 0 {
            return i + (x.trailing_zeros() / 8) as usize;
        }
        i += 8;
    }
    while i < MAX_MATCH && a[i] == b[i] {
        i += 1;
    }
    i
}

pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}

pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    static T: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = T.get_or_init(|| {
        let mut t = [0u32; 256];
        for (n, e) in t.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    let mut c = !crc;
    for &b in data {
        c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// `zlib.compress(data, 9)`.
pub fn compress9(data: &[u8]) -> Vec<u8> {
    let mut s = State {
        input: data,
        in_pos: 0,
        window: vec![0u8; WINDOW_SIZE].into_boxed_slice().try_into().unwrap(),
        prev: vec![0u16; WSIZE].into_boxed_slice().try_into().unwrap(),
        head: vec![0u16; HASH_SIZE].into_boxed_slice().try_into().unwrap(),
        ins_h: 0,
        block_start: 0,
        match_length: MIN_MATCH - 1,
        prev_match: 0,
        match_available: false,
        strstart: 0,
        match_start: 0,
        lookahead: 0,
        prev_length: MIN_MATCH - 1,
        insert: 0,
        ltree: Tree { fc: vec![0; HEAP_SIZE], dl: vec![0; HEAP_SIZE], max_code: 0 },
        dtree: Tree { fc: vec![0; 2 * D_CODES + 1], dl: vec![0; 2 * D_CODES + 1], max_code: 0 },
        bltree: Tree { fc: vec![0; 2 * BL_CODES + 1], dl: vec![0; 2 * BL_CODES + 1], max_code: 0 },
        bl_count: [0; MAX_BITS + 1],
        heap: [0; HEAP_SIZE],
        heap_len: 0,
        heap_max: 0,
        depth: [0; HEAP_SIZE],
        sym_buf: vec![0; LIT_BUFSIZE * 3],
        sym_next: 0,
        opt_len: 0,
        static_len: 0,
        bits: BitOut { out: Vec::with_capacity(data.len() / 2 + 64), buf: 0, n: 0 },
    };
    s.bits.out.extend_from_slice(&[0x78, 0xDA]);
    s.init_block();
    s.deflate_slow();
    s.bits.out.extend_from_slice(&adler32(data).to_be_bytes());
    s.bits.out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal inflater (stored, fixed and dynamic blocks) to check the round trip.
    pub fn inflate(z: &[u8]) -> Vec<u8> {
        let d = &z[2..];
        let mut out = Vec::new();
        let (mut pos, mut bit) = (0usize, 0u32);
        let mut get = |n: u32| -> u32 {
            let mut v = 0;
            for i in 0..n {
                let b = (d[pos] >> bit) & 1;
                v |= (b as u32) << i;
                bit += 1;
                if bit == 8 {
                    bit = 0;
                    pos += 1;
                }
            }
            v
        };
        fn decode(get: &mut dyn FnMut(u32) -> u32, lens: &[u8]) -> Vec<(u32, u32, usize)> {
            let mut codes = Vec::new();
            let mut code = 0u32;
            for l in 1..16u32 {
                for (s, &ln) in lens.iter().enumerate() {
                    if ln as u32 == l {
                        codes.push((code, l, s));
                        code += 1;
                    }
                }
                code <<= 1;
            }
            let _ = get;
            codes
        }
        fn read_sym(get: &mut dyn FnMut(u32) -> u32, codes: &[(u32, u32, usize)]) -> usize {
            let (mut code, mut len) = (0u32, 0u32);
            loop {
                code = (code << 1) | get(1);
                len += 1;
                if let Some(c) = codes.iter().find(|c| c.1 == len && c.0 == code) {
                    return c.2;
                }
            }
        }
        let t = tables();
        loop {
            let last = get(1);
            let ty = get(2);
            if ty == 0 {
                panic!("stored blocks not needed by the test");
            }
            let (lc, dc) = if ty == 1 {
                let mut l = [0u8; 288];
                for (n, x) in l.iter_mut().enumerate() {
                    *x = t.static_ltree[n.min(L_CODES + 1)].1 as u8;
                }
                (decode(&mut get, &l), decode(&mut get, &[5u8; 30]))
            } else {
                let hlit = get(5) as usize + 257;
                let hdist = get(5) as usize + 1;
                let hclen = get(4) as usize + 4;
                let mut bl = [0u8; 19];
                for i in 0..hclen {
                    bl[BL_ORDER[i]] = get(3) as u8;
                }
                let blc = decode(&mut get, &bl);
                let mut lens = Vec::new();
                while lens.len() < hlit + hdist {
                    let s = read_sym(&mut get, &blc);
                    match s {
                        0..=15 => lens.push(s as u8),
                        16 => {
                            let p = *lens.last().unwrap();
                            for _ in 0..3 + get(2) {
                                lens.push(p)
                            }
                        }
                        17 => {
                            for _ in 0..3 + get(3) {
                                lens.push(0)
                            }
                        }
                        _ => {
                            for _ in 0..11 + get(7) {
                                lens.push(0)
                            }
                        }
                    }
                }
                (decode(&mut get, &lens[..hlit]), decode(&mut get, &lens[hlit..]))
            };
            loop {
                let s = read_sym(&mut get, &lc);
                if s < 256 {
                    out.push(s as u8);
                } else if s == 256 {
                    break;
                } else {
                    let c = s - 257;
                    let len = t.base_length[c] as usize + get(EXTRA_LBITS[c]) as usize + 3;
                    let dcode = read_sym(&mut get, &dc);
                    let dist = t.base_dist[dcode] as usize + get(EXTRA_DBITS[dcode]) as usize + 1;
                    for _ in 0..len {
                        out.push(out[out.len() - dist]);
                    }
                }
            }
            if last == 1 {
                break;
            }
        }
        out
    }

    /// CRC-32 (IEEE 802.3, zlib's): the manifest's file checksum.
    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32_update(crc32(b"1234"), b"56789"), 0xCBF4_3926);
    }

    #[test]
    fn round_trip() {
        let mut data = Vec::new();
        let mut x = 12345u32;
        for i in 0..200_000u32 {
            x = x.wrapping_mul(1103515245).wrapping_add(12345);
            data.push(if i % 7 < 4 { (i / 100) as u8 } else { (x >> 24) as u8 & 0x0F });
        }
        let z = compress9(&data);
        assert_eq!(&z[..2], &[0x78, 0xDA]);
        assert_eq!(inflate(&z), data);
        assert_eq!(crc32(b"123456789"), 0xCBF43926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E60398);
    }
}
