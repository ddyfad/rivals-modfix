//! Minimal Oodle encoder: one Mermaid block (decoder type 10). LZ with near (16-bit) offsets,
//! which is enough for IoStore's 64/128 KB compression blocks, and Huffman-coded literal,
//! command and offset streams. Callers decompress the result and compare it before using it.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

const HASH_BITS: u32 = 15;
const MAX_CHAIN: usize = 128;

/// Compress one block. None when the input doesn't fit the format or wouldn't get smaller.
pub fn compress(src: &[u8]) -> Option<Vec<u8>> {
    let n = src.len();
    if n < 128 || n > 0x20000 { return None; } // Oodle rejects LZ chunks under 128 bytes
    let (mut lits, mut cmds, mut offs, mut lens) = (Vec::new(), Vec::new(), Vec::<u16>::new(), Vec::new());
    let mut head = vec![-1i32; 1 << HASH_BITS];
    let mut prev = vec![-1i32; n];
    let hash = |p: usize| (u32::from_le_bytes(src[p..p + 4].try_into().unwrap()).wrapping_mul(0x9E3779B1) >> (32 - HASH_BITS)) as usize;
    let insert = |p: usize, head: &mut [i32], prev: &mut [i32]| { if p + 4 <= n { let h = hash(p); prev[p] = head[h]; head[h] = p as i32; } };
    let match_len = |a: usize, b: usize, max: usize| (0..max).take_while(|&i| src[a + i] == src[b + i]).count();
    let find = |pos: usize, max: usize, head: &[i32], prev: &[i32]| {
        let (mut best, mut dist) = (0, 0);
        let mut cand = head[hash(pos)];
        for _ in 0..MAX_CHAIN {
            if cand < 0 || pos - cand as usize > 0xFFFF { break; }
            let d = pos - cand as usize;
            if d >= 8 { let l = match_len(pos, cand as usize, max); if l > best { best = l; dist = d; if l == max { break; } } }
            cand = prev[cand as usize];
        }
        (best, dist)
    };

    // The first 8 bytes of a stream are stored as-is.
    for p in 0..8 { insert(p, &mut head, &mut prev); }
    let mut recent = 8usize;
    let mut first_half_cmds = 0;
    // The decoder runs each 64 KB half separately: commands can't cross the boundary, and the
    // rest of each half is filled from the literal stream.
    for (half_start, half_end) in [(8, n.min(0x10000)), (0x10000, n)] {
        if half_start >= half_end { break; }
        // The decoder copies 8 and 16 bytes at a time, so matches stay clear of the last 16 bytes.
        let end = half_end.saturating_sub(16);
        let (mut pos, mut lit_start) = (half_start, half_start);
        while pos + 4 <= end {
            let rep = match_len(pos, pos - recent, end - pos);
            let (best, dist) = find(pos, end - pos, &head, &prev);
            let (len, d, is_rep) = if rep >= 3 && rep + 2 >= best { (rep, recent, true) } else if best >= 5 { (best, dist, false) } else { (0, 0, false) };
            // Lazy matching: a clearly longer match one byte on is worth a literal.
            let lazy = !is_rep && len > 0 && pos + 5 <= end && find(pos + 1, end - pos - 1, &head, &prev).0 > len + 1;
            if len == 0 || lazy { insert(pos, &mut head, &mut prev); pos += 1; continue; }

            lits.extend_from_slice(&src[lit_start..pos]);
            let mut lit = pos - lit_start;
            if lit >= 64 { cmds.push(0); push_len(&mut lens, lit - 64); lit = 0; }
            while lit > 7 { cmds.push(0x87); lit -= 7; }
            let mut left = len;
            if len >= 91 {
                if lit > 0 { cmds.push(0x80 | lit as u8); }
                cmds.push(1); offs.push(d as u16); push_len(&mut lens, len - 91);
                left = 0;
            } else {
                let m = left.min(15);
                cmds.push(if is_rep { 0x80 } else { offs.push(d as u16); 0 } | (m << 3) as u8 | lit as u8);
                left -= m;
            }
            while left > 0 { let m = left.min(15); cmds.push(0x80 | (m << 3) as u8); left -= m; }
            for p in pos..pos + len { insert(p, &mut head, &mut prev); }
            pos += len; lit_start = pos; recent = d;
        }
        lits.extend_from_slice(&src[lit_start..half_end]);
        if half_start == 8 { first_half_cmds = cmds.len(); }
    }

    let mut body = src[..8].to_vec();
    put_stream(&mut body, &lits);
    put_stream(&mut body, &cmds);
    if n > 0x10000 { body.extend_from_slice(&(first_half_cmds as u16).to_le_bytes()); }
    // Offsets go either as a plain u16 list or as entropy-coded high and low bytes.
    let mut split = vec![0xFF, 0xFF];
    put_stream(&mut split, &offs.iter().map(|o| (o >> 8) as u8).collect::<Vec<_>>());
    put_stream(&mut split, &offs.iter().map(|o| *o as u8).collect::<Vec<_>>());
    if offs.len() < 0xFFFF && split.len() >= 2 + offs.len() * 2 {
        body.extend_from_slice(&(offs.len() as u16).to_le_bytes());
        for o in &offs { body.extend_from_slice(&o.to_le_bytes()); }
    } else {
        body.extend_from_slice(&split);
    }
    body.extend_from_slice(&[0, 0, 0]); // no far offsets
    body.extend_from_slice(&lens);

    let quantum = 3 + body.len();
    if 2 + 3 + quantum >= n { return None; }
    let mut out = vec![0x8C, 0x0A];
    out.extend_from_slice(&((quantum - 1) as u32).to_be_bytes()[1..]);
    out.extend_from_slice(&(0x800000 | 1 << 19 | body.len() as u32).to_be_bytes()[1..]); // LZ chunk, plain literals
    out.extend_from_slice(&body);
    Some(out)
}

fn push_len(lens: &mut Vec<u8>, v: usize) {
    if v <= 251 { lens.push(v as u8); return; }
    let b = 252 + (v - 252) % 4;
    lens.push(b as u8);
    lens.extend_from_slice(&(((v - b) / 4) as u16).to_le_bytes());
}

/// Append a byte stream, Huffman-coded when that's smaller than storing it.
fn put_stream(out: &mut Vec<u8>, data: &[u8]) {
    if let Some(h) = huffman_stream(data).filter(|h| h.len() < 3 + data.len()) {
        out.extend_from_slice(&h);
    } else {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes()[1..]);
        out.extend_from_slice(data);
    }
}

/// MSB-first bit writer, used for the code length table.
struct MsbBits { out: Vec<u8>, acc: u64, n: u32 }
impl MsbBits {
    fn put(&mut self, v: u32, bits: u32) {
        self.acc = self.acc << bits | v as u64 & ((1u64 << bits) - 1); self.n += bits;
        while self.n >= 8 { self.n -= 8; self.out.push((self.acc >> self.n) as u8); }
    }
    fn gamma(&mut self, v: u32) { // v >= 1
        let x = v + 1; let k = 31 - x.leading_zeros();
        self.put(0, k - 1); self.put(x, k + 1);
    }
    fn finish(mut self) -> Vec<u8> { if self.n > 0 { self.put(0, 8 - self.n); } self.out }
}

/// LSB-first bit writer, used for the Huffman-coded symbols.
struct LsbBits { out: Vec<u8>, acc: u64, n: u32 }
impl LsbBits {
    fn put(&mut self, v: u32, bits: u32) {
        self.acc |= (v as u64) << self.n; self.n += bits;
        while self.n >= 8 { self.out.push(self.acc as u8); self.acc >>= 8; self.n -= 8; }
    }
    fn finish(mut self) -> Vec<u8> { if self.n > 0 { self.out.push(self.acc as u8); } self.out }
}

/// Code lengths (at most 11 bits) for every byte value that occurs.
fn code_lengths(data: &[u8]) -> [u8; 256] {
    let mut freq = [0u32; 256];
    for &b in data { freq[b as usize] += 1; }
    loop {
        let mut heap: BinaryHeap<Reverse<(u32, usize)>> = (0..256).filter(|&s| freq[s] > 0).map(|s| Reverse((freq[s], s))).collect();
        let mut parent = vec![usize::MAX; 256];
        while heap.len() > 1 {
            let (Reverse((a, x)), Reverse((b, y))) = (heap.pop().unwrap(), heap.pop().unwrap());
            parent.push(usize::MAX);
            let id = parent.len() - 1;
            parent[x] = id; parent[y] = id;
            heap.push(Reverse((a + b, id)));
        }
        let mut lens = [0u8; 256];
        for s in (0..256).filter(|&s| freq[s] > 0) {
            let (mut d, mut p) = (0, s);
            while parent[p] != usize::MAX { p = parent[p]; d += 1; }
            lens[s] = d.min(255) as u8;
        }
        if lens.iter().all(|&l| l <= 11) { return lens; }
        for f in freq.iter_mut() { if *f > 0 { *f = (*f + 1) / 2; } }
    }
}

/// Huffman stream (Kraken_DecodeBytes chunk type 2): code length table, then the symbols spread
/// over three bitstreams (forward, forward from the middle, backward from the end).
fn huffman_stream(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 32 || data.len() > 0x40000 { return None; }
    let lens = code_lengths(data);
    let used: Vec<usize> = (0..256).filter(|&s| lens[s] > 0).collect();
    if used.len() < 2 { return None; }

    // Canonical codes: shorter codes first, then by symbol.
    let mut order = used.clone();
    order.sort_by_key(|&s| (lens[s], s));
    let mut codes = [0u32; 256];
    let mut slot = 0u32;
    for &s in &order {
        let l = lens[s] as u32;
        codes[s] = (slot >> (11 - l)).reverse_bits() >> (32 - l); // stream bits run MSB of the code first
        slot += 1 << (11 - l);
    }

    let mut table = (0..4).filter_map(|forced| gamma_table(&lens, forced)).min_by_key(|t| t.len()).unwrap_or_default();
    if used.len() < 256 {
        let mut w = MsbBits { out: Vec::new(), acc: 0, n: 0 };
        w.put(0, 2); // old format, sparse
        w.put(used.len() as u32, 8); w.put(4, 3);
        for &s in &used { w.put(s as u32, 8); w.put(lens[s] as u32 - 1, 4); }
        let t = w.finish();
        if table.is_empty() || t.len() < table.len() { table = t; }
    }

    let mut streams = [LsbBits { out: Vec::new(), acc: 0, n: 0 }, LsbBits { out: Vec::new(), acc: 0, n: 0 }, LsbBits { out: Vec::new(), acc: 0, n: 0 }];
    for (i, &b) in data.iter().enumerate() { streams[[0, 2, 1][i % 3]].put(codes[b as usize], lens[b as usize] as u32); }
    let [a, mid, end] = streams.map(|s| s.finish());
    if a.len() > 0xFFFF { return None; }

    let mut body = table;
    body.extend_from_slice(&(a.len() as u16).to_le_bytes());
    body.extend_from_slice(&a);
    body.extend_from_slice(&mid);
    body.extend(end.iter().rev());
    if body.len() >= data.len() { return None; }
    let d = data.len() as u32 - 1;
    let mut out = vec![0x20 | (d >> 14) as u8];
    out.extend_from_slice(&((d & 0x3FFF) << 18 | body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Some(out)
}

/// Code length table in the old format's gamma mode: runs of unused symbols, runs of used ones,
/// and each length as a zigzag delta from a running average. None if a delta is too big for
/// this `forced` setting.
fn gamma_table(lens: &[u8; 256], forced: u32) -> Option<Vec<u8>> {
    let mut w = MsbBits { out: Vec::new(), acc: 0, n: 0 };
    w.put(1, 2); // old format, gamma
    w.put(forced, 2);
    let mut avg_x4 = 32i32;
    let mut s = 0;
    w.put((lens[0] > 0) as u32, 1);
    while s < 256 {
        if lens[s] == 0 {
            let start = s; while s < 256 && lens[s] == 0 { s += 1; }
            w.gamma((s - start) as u32);
            if s == 256 { break; }
        }
        let start = s; while s < 256 && lens[s] > 0 { s += 1; }
        w.gamma((s - start) as u32);
        for &l in &lens[start..s] {
            let delta = l as i32 - ((avg_x4 + 2) >> 2);
            let v = if delta >= 0 { 2 * delta } else { -2 * delta - 1 } as u32;
            if v >> forced > 20 >> forced { return None; }
            w.put(0, v >> forced); w.put(1, 1); w.put(v, forced);
            avg_x4 = l as i32 + ((3 * avg_x4 + 2) >> 2);
        }
    }
    Some(w.finish())
}
