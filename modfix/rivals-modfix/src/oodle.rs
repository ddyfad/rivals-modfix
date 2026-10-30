//! Minimal Oodle encoder: one Mermaid block (decoder type 10) with every stream stored raw,
//! i.e. what Selkie produces. Only near (16-bit) offsets and short/long tokens are used, which
//! is enough for IoStore's 64/128 KB compression blocks. Callers decompress the result and
//! compare it before using it.

const HASH_BITS: u32 = 14;
const MAX_CHAIN: usize = 32;

/// Compress one block. None when the input doesn't fit the format or wouldn't get smaller.
pub fn compress(src: &[u8]) -> Option<Vec<u8>> {
    let n = src.len();
    if n < 128 || n > 0x20000 { return None; } // Oodle rejects LZ chunks under 128 bytes
    let (mut lits, mut cmds, mut offs, mut lens) = (Vec::new(), Vec::new(), Vec::<u16>::new(), Vec::new());
    let mut head = vec![-1i32; 1 << HASH_BITS];
    let mut prev = vec![-1i32; n];
    let hash = |p: usize| (u32::from_le_bytes(src[p..p + 4].try_into().unwrap()).wrapping_mul(0x9E3779B1) >> (32 - HASH_BITS)) as usize;
    let insert = |p: usize, head: &mut [i32], prev: &mut [i32]| { let h = hash(p); prev[p] = head[h]; head[h] = p as i32; };
    let match_len = |a: usize, b: usize, max: usize| (0..max).take_while(|&i| src[a + i] == src[b + i]).count();

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
            let max = end - pos;
            let rep = match_len(pos, pos - recent, max);
            let (mut best, mut dist) = (0, 0);
            let mut cand = head[hash(pos)];
            for _ in 0..MAX_CHAIN {
                if cand < 0 || pos - cand as usize > 0xFFFF { break; }
                let d = pos - cand as usize;
                if d >= 8 { let l = match_len(pos, cand as usize, max); if l > best { best = l; dist = d; } }
                cand = prev[cand as usize];
            }
            let (len, d, is_rep) = if rep >= 3 && rep + 2 >= best { (rep, recent, true) } else if best >= 5 { (best, dist, false) } else { (0, 0, false) };
            if len == 0 { insert(pos, &mut head, &mut prev); pos += 1; continue; }

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
            for p in pos..pos + len { if p + 4 <= n { insert(p, &mut head, &mut prev); } }
            pos += len; lit_start = pos; recent = d;
    }
    lits.extend_from_slice(&src[lit_start..half_end]);
    if half_start == 8 { first_half_cmds = cmds.len(); }
    }

    let mut body = src[..8].to_vec();
    for s in [&lits, &cmds] { body.extend_from_slice(&(s.len() as u32).to_be_bytes()[1..]); body.extend_from_slice(s); }
    if n > 0x10000 { body.extend_from_slice(&(first_half_cmds as u16).to_le_bytes()); }
    body.extend_from_slice(&(offs.len() as u16).to_le_bytes());
    for o in &offs { body.extend_from_slice(&o.to_le_bytes()); }
    body.extend_from_slice(&[0, 0, 0]); // no far offsets
    body.extend_from_slice(&lens);

    let quantum = 3 + body.len();
    if 2 + 3 + quantum >= n { return None; }
    let mut out = vec![0x8C, 0x0A];
    out.extend_from_slice(&((quantum - 1) as u32).to_be_bytes()[1..]);
    out.extend_from_slice(&(0x800000 | 1 << 19 | body.len() as u32).to_be_bytes()[1..]); // LZ chunk, raw literals
    out.extend_from_slice(&body);
    Some(out)
}

fn push_len(lens: &mut Vec<u8>, v: usize) {
    if v <= 251 { lens.push(v as u8); return; }
    let b = 252 + (v - 252) % 4;
    lens.push(b as u8);
    lens.extend_from_slice(&(((v - b) / 4) as u16).to_le_bytes());
}
