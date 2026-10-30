//! Core of rivals-modfix: IoStore container reading/writing, zen package patching and the
//! unversioned-property walker. Used by the Windows exe (main.rs) and the WebAssembly build
//! for the crash finder page (bottom of this file).

use aes::cipher::{BlockDecrypt, KeyInit};
use serde::Deserialize;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::path::Path;

pub mod oodle;

pub const MIC_CLASS: u64 = 0x7f7253ab5570e48f; // FPackageObjectIndex of /Script/Engine.MaterialInstanceConstant
pub const STRUCT: &str = "MaterialInstanceBasePropertyOverrides";
pub const INSERT_AT: u32 = 40;
pub const INSERT_N: u32 = 2;
/// Steam's "LastUpdated" for the Oct 30 2026 build; mod files older than this are assumed to be old-layout
/// when a material reads cleanly both ways.
pub const UPDATE_TIME: u64 = 1793311882;
/// The game's public AES key. Encrypted mod containers use it with plain AES-256-ECB
/// (NetEase's custom scheme only applies to legacy .pak files).
const AES_KEY: [u8; 32] = [0x0C, 0x26, 0x3D, 0x8C, 0x22, 0xDC, 0xB0, 0x85, 0x89, 0x48, 0x99, 0xC3, 0xA3, 0x79, 0x63, 0x83,
                           0xE9, 0xBF, 0x9D, 0xE0, 0xCB, 0xFB, 0x08, 0xC9, 0xBF, 0x2D, 0xEF, 0x2E, 0x84, 0xF2, 0x9D, 0x74];

fn decrypt(buf: &mut [u8]) {
    let c = aes::Aes256::new(&AES_KEY.into());
    for b in buf.chunks_exact_mut(16) { c.decrypt_block(b.into()); }
}

#[derive(Deserialize, Clone)]
#[serde(tag = "t")]
enum Ty {
    Struct { s: String },
    Array { i: Box<Ty> },
    Set { i: Box<Ty> },
    Optional { i: Box<Ty> },
    Enum { i: Box<Ty> },
    Map { k: Box<Ty>, v: Box<Ty> },
    #[serde(untagged)]
    Prim { t: u32 },
}
#[derive(Deserialize, Clone)]
struct StructDef { #[serde(rename = "super")] sup: Option<String>, count: u32, props: Vec<(u32, String, u32, Ty)> }
#[derive(Deserialize)]
pub struct SchemaFile { old: HashMap<String, StructDef>, new: HashMap<String, StructDef> }
struct Schema(HashMap<String, StructDef>);

fn load_schemas() -> (Schema, Schema) {
    let f: SchemaFile = serde_json::from_str(include_str!("schema.json")).expect("schema.json");
    let mut new = f.old.clone();
    for (k, v) in f.new { new.insert(k, v); }
    (Schema(f.old), Schema(new))
}

fn native_size(name: &str) -> Option<usize> {
    Some(match name {
        "Guid" | "LinearColor" | "Vector2D" | "Quat4f" | "Vector4f" | "IntVector4" => 16,
        "Color" | "FrameNumber" => 4, "Vector2f" | "IntPoint" | "DateTime" | "Timespan" | "FrameRate" => 8,
        "Vector3f" | "IntVector" => 12, "Vector" | "Rotator" => 24, "Vector4" | "Quat" | "Plane" => 32,
        "Transform" => 80, "Matrix" => 128, "Box" => 49, "Box2D" => 33, "BoxSphereBounds" => 56,
        _ => return None,
    })
}

struct Walker<'a> { schema: &'a Schema, b: &'a [u8], marks: Vec<(usize, usize)> /* (start,end) of STRUCT headers */ }

#[derive(Debug)]
struct WalkErr(String);
type R<T> = Result<T, WalkErr>;
fn err<T>(s: impl Into<String>) -> R<T> { Err(WalkErr(s.into())) }

impl<'a> Walker<'a> {
    fn u16(&self, p: usize) -> R<u16> { self.b.get(p..p + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or(WalkErr("eof".into())) }
    fn i32(&self, p: usize) -> R<i32> { self.b.get(p..p + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap())).ok_or(WalkErr("eof".into())) }
    fn need(&self, p: usize, n: usize) -> R<usize> { if p + n <= self.b.len() { Ok(p + n) } else { err("eof") } }

    fn slots(&self, cls: &str) -> R<HashMap<u32, (String, Ty)>> {
        // Schema order: the class itself first, then each super class in turn.
        let mut out = HashMap::new();
        let mut base = 0u32;
        let mut cur = Some(cls.to_string());
        while let Some(c) = cur {
            let d = self.schema.0.get(&c).ok_or(WalkErr(format!("unknown struct {c}")))?;
            for (idx, name, asz, t) in &d.props {
                for k in 0..*asz { out.insert(base + idx + k, (name.clone(), t.clone())); }
            }
            base += d.count;
            cur = d.sup.clone();
        }
        Ok(out)
    }

    fn header(&self, mut p: usize) -> R<(Vec<(u32, bool, u32)>, u128, usize)> {
        let mut frags = Vec::new();
        loop {
            let v = self.u16(p)?; p += 2;
            frags.push(((v & 0x7f) as u32, v & 0x80 != 0, (v >> 9) as u32));
            if v & 0x100 != 0 { break; }
            if frags.len() > 64 { return err("runaway header"); }
        }
        let nz: u32 = frags.iter().filter(|f| f.1).map(|f| f.2).sum();
        let mut mask = 0u128;
        if nz > 0 {
            let w = if nz <= 8 { 1 } else if nz <= 16 { 2 } else { 4 * ((nz as usize + 31) / 32) };
            if w > 16 { return err("zero mask too wide"); }
            let end = self.need(p, w)?;
            for (i, byte) in self.b[p..end].iter().enumerate() { mask |= (*byte as u128) << (8 * i); }
            p = end;
        }
        Ok((frags, mask, p))
    }

    fn walk_struct(&mut self, cls: &str, mut p: usize) -> R<usize> {
        let slots = self.slots(cls)?;
        let h0 = p;
        let (frags, mask, np) = self.header(p)?; p = np;
        if cls == STRUCT { self.marks.push((h0, p)); }
        let (mut idx, mut zbit) = (0u32, 0u32);
        for (skip, hz, nval) in frags {
            idx += skip;
            for _ in 0..nval {
                let zero = if hz { let z = (mask >> zbit) & 1 == 1; zbit += 1; z } else { false };
                let (_name, t) = slots.get(&idx).ok_or(WalkErr(format!("{cls}: slot {idx} not serialisable")))?;
                if !zero { p = self.value(t, p)?; }
                idx += 1;
            }
        }
        Ok(p)
    }

    fn value(&mut self, t: &Ty, p: usize) -> R<usize> {
        match t {
            Ty::Prim { t: code } => match code {
                0 | 1 | 23 => self.need(p, 1), 20 | 22 => self.need(p, 2),
                2 | 3 | 4 | 12 | 14 | 19 => self.need(p, 4), 5 | 7 | 18 | 21 => self.need(p, 8),
                15 => self.need(p, 16), 6 => self.need(p, 12),
                10 | 29 | 30 => self.fstring(p),
                16 | 17 => { let q = self.need(p, 16)?; self.fstring(q) } // FTopLevelAssetPath + sub path
                13 => { let n = self.i32(p)?; self.need(p + 4, 12 * n.max(0) as usize) }
                _ => err(format!("unsupported property type {code}")),
            },
            Ty::Enum { i } => self.value(i, p),
            Ty::Struct { s } => {
                if let Some(n) = native_size(s) { return self.need(p, n); }
                if s == "SoftObjectPath" || s == "SoftClassPath" { return self.value(&Ty::Prim { t: 17 }, p); }
                self.walk_struct(s, p)
            }
            Ty::Array { i } | Ty::Set { i } => {
                let n = self.i32(p)?; if n < 0 || n > 1_000_000 { return err("bad array count"); }
                let mut q = p + 4;
                for _ in 0..n { q = self.value(i, q)?; }
                Ok(q)
            }
            Ty::Map { k, v } => {
                let n = self.i32(p + 4)?; if n < 0 || n > 1_000_000 { return err("bad map count"); }
                let mut q = p + 8;
                for _ in 0..n { q = self.value(k, q)?; q = self.value(v, q)?; }
                Ok(q)
            }
            Ty::Optional { i } => { if self.i32(p)? != 0 { self.value(i, p + 4) } else { Ok(p + 4) } }
        }
    }
    fn fstring(&self, p: usize) -> R<usize> {
        let n = self.i32(p)?;
        self.need(p + 4, if n < 0 { (-(n as i64) * 2) as usize } else { n as usize })
    }
}

/// Walk a MaterialInstanceConstant export. Ok(end) means every property parsed and the
/// next field (UObject's "has guid" flag) is zero, which a wrong layout almost never gives.
fn walk_mic(schema: &Schema, blob: &[u8]) -> R<(usize, Vec<(usize, usize)>)> {
    let mut w = Walker { schema, b: blob, marks: Vec::new() };
    let end = w.walk_struct("MaterialInstanceConstant", 0)?;
    if end + 4 > blob.len() || blob[end..end + 4] != [0, 0, 0, 0] { return err("property stream does not end cleanly"); }
    Ok((end, w.marks))
}

/// Insert `n` skipped slots at schema index `at` into the unversioned header b[h0..h1].
fn shift_header(b: &[u8], h0: usize, h1: usize, at: u32, n: u32) -> R<Vec<u8>> {
    let mut frags: Vec<(u32, bool, u32, bool)> = Vec::new();
    let mut p = h0;
    loop {
        let v = u16::from_le_bytes([b[p], b[p + 1]]); p += 2;
        frags.push(((v & 0x7f) as u32, v & 0x80 != 0, (v >> 9) as u32, v & 0x100 != 0));
        if v & 0x100 != 0 { break; }
    }
    let mask = &b[p..h1];
    let (mut out, mut idx, mut done) = (Vec::new(), 0u32, false);
    for (skip, hz, nval, last) in frags {
        if !done && idx + skip >= at { out.push((skip + n, hz, nval, last)); done = true; }
        else if !done && idx + skip + nval > at {
            let before = at - idx - skip;
            out.push((skip, hz, before, false)); out.push((n, hz, nval - before, last)); done = true;
        } else { out.push((skip, hz, nval, last)); }
        idx += skip + nval;
    }
    if !done { return Ok(b[h0..h1].to_vec()); }
    let mut hdr = Vec::new();
    for (skip, hz, nval, last) in out {
        if skip > 127 || nval > 127 { return err("fragment overflow"); }
        let v = skip as u16 | if hz { 0x80 } else { 0 } | if last { 0x100 } else { 0 } | ((nval as u16) << 9);
        hdr.extend_from_slice(&v.to_le_bytes());
    }
    hdr.extend_from_slice(mask);
    Ok(hdr)
}

struct ZenExport { entry: usize, off: u64, size: u64, class: u64 }
fn zen_exports(pkg: &[u8]) -> Option<(usize, Vec<ZenExport>)> {
    if pkg.len() < 52 { return None; }
    let u32at = |o: usize| u32::from_le_bytes(pkg[o..o + 4].try_into().unwrap());
    if u32at(0) != 0 { return None; } // versioned packages are not handled
    let hsize = u32at(4) as usize;
    let (exp_map, bundle) = (u32at(32) as usize, u32at(36) as usize);
    if bundle < exp_map || bundle > hsize || (bundle - exp_map) % 72 != 0 { return None; }
    let mut out = Vec::new();
    for i in 0..(bundle - exp_map) / 72 {
        let q = exp_map + i * 72;
        let u64at = |o: usize| u64::from_le_bytes(pkg[o..o + 8].try_into().unwrap());
        out.push(ZenExport { entry: q, off: u64at(q), size: u64at(q + 8), class: u64at(q + 32) });
    }
    Some((hsize, out))
}

enum Outcome { NoMaterials, UpToDate(usize), Fixed(usize, Vec<u8>), Skipped(String) }

/// Patch every stale MaterialInstanceConstant export in a zen package.
fn fix_package(pkg: &[u8], old: &Schema, new: &Schema, assume_old: bool) -> Outcome {
    let Some((hsize, exports)) = zen_exports(pkg) else { return Outcome::Skipped("unreadable package summary".into()) };
    let mics: Vec<&ZenExport> = exports.iter().filter(|e| e.class == MIC_CLASS).collect();
    if mics.is_empty() { return Outcome::NoMaterials; }
    let mut data = pkg.to_vec();
    let mut fixed = 0usize;
    let mut shift: i64 = 0; // bytes inserted so far, applies to exports after the patched ones
    // Export data is laid out in offset order, so process in that order to keep offsets right.
    let mut order: Vec<usize> = (0..exports.len()).collect();
    order.sort_by_key(|&i| exports[i].off);
    let mut new_sizes: HashMap<usize, u64> = HashMap::new();
    for &i in &order {
        let e = &exports[i];
        if e.class != MIC_CLASS { continue; }
        let start = hsize + (e.off as i64 + shift) as usize;
        let end = start + e.size as usize;
        if end > data.len() { return Outcome::Skipped("export outside package".into()); }
        let blob = data[start..end].to_vec();
        let old_ok = walk_mic(old, &blob);
        let new_ok = walk_mic(new, &blob);
        let (old_end, marks) = match (&old_ok, &new_ok) {
            (Err(_), Ok(_)) => continue,                                   // already in the new layout
            (Err(a), Err(_)) => return Outcome::Skipped(format!("cannot read material: {}", a.0)),
            (Ok(o), Err(_)) => o.clone(),
            (Ok(o), Ok(_)) => {
                // Both layouts read cleanly: only bools/enums past the insertion point. Decide by file date.
                if o.1.is_empty() { continue; }
                if !assume_old { continue; }
                o.clone()
            }
        };
        if marks.is_empty() { continue; } // no overrides serialised, nothing to shift
        let (h0, h1) = marks[0];
        let hdr = match shift_header(&blob, h0, h1, INSERT_AT, INSERT_N) { Ok(h) => h, Err(e) => return Outcome::Skipped(e.0) };
        if hdr == blob[h0..h1] { continue; }
        let mut patched = blob[..h0].to_vec(); patched.extend_from_slice(&hdr); patched.extend_from_slice(&blob[h1..]);
        let delta = patched.len() as i64 - blob.len() as i64;
        match walk_mic(new, &patched) {
            Ok((e2, _)) if e2 as i64 == old_end as i64 + delta && patched[e2..] == blob[old_end..] => {}
            _ => return Outcome::Skipped("patched material does not read back correctly".into()),
        }
        data.splice(start..end, patched);
        new_sizes.insert(i, (e.size as i64 + delta) as u64);
        shift += delta; fixed += 1;
    }
    if fixed == 0 { return Outcome::UpToDate(mics.len()); }
    // Rewrite the export map: new sizes, and offsets of everything after a grown export.
    let mut running: i64 = 0;
    for &i in &order {
        let e = &exports[i];
        let off = (e.off as i64 + running) as u64;
        data[e.entry..e.entry + 8].copy_from_slice(&off.to_le_bytes());
        if let Some(sz) = new_sizes.get(&i) {
            data[e.entry + 8..e.entry + 16].copy_from_slice(&sz.to_le_bytes());
            running += *sz as i64 - e.size as i64;
        }
    }
    Outcome::Fixed(fixed, data)
}

pub struct Toc {
    pub raw: Vec<u8>, pub header_size: usize, pub ver: u8, pub flags: u8, pub cbs: usize,
    pub ids: Vec<[u8; 12]>, pub offlen: Vec<(u64, u64)>,
    pub blocks: Vec<(u64, u32, u32, u8)>, // file offset, compressed size, uncompressed size, method
    pub methods_off: usize, pub methods_len: usize, pub dir_off: usize, pub dir_len: usize, pub meta_off: usize,
}
pub fn read_toc(path: &Path) -> Result<Toc, String> { parse_toc(std::fs::read(path).map_err(|e| e.to_string())?) }
pub fn parse_toc(raw: Vec<u8>) -> Result<Toc, String> {
    if raw.len() < 144 || &raw[..16] != b"-==--==--==--==-" { return Err("not a .utoc".into()); }
    let u32at = |o: usize| u32::from_le_bytes(raw[o..o + 4].try_into().unwrap()) as usize;
    let (ver, hdr) = (raw[16], u32at(20));
    let (n, nblk, blksz, ncm, cml, cbs, dirsz) = (u32at(24), u32at(28), u32at(32), u32at(36), u32at(40), u32at(44), u32at(48));
    let flags = raw[80]; let nseed = u32at(84); let nnoph = u32at(96);
    if blksz != 12 { return Err("unexpected compression block entry size".into()); }
    let mut o = hdr;
    let ids: Vec<[u8; 12]> = (0..n).map(|i| raw[o + i * 12..o + i * 12 + 12].try_into().unwrap()).collect(); o += n * 12;
    let offlen: Vec<(u64, u64)> = (0..n).map(|i| { let e = &raw[o + i * 10..o + i * 10 + 10];
        let be5 = |s: &[u8]| s.iter().fold(0u64, |a, &b| (a << 8) | b as u64);
        (be5(&e[0..5]), be5(&e[5..10])) }).collect(); o += n * 10;
    if ver >= 4 { o += nseed * 4; }
    if ver >= 5 { o += nnoph * 4; }
    let blocks: Vec<(u64, u32, u32, u8)> = (0..nblk).map(|i| { let e = &raw[o + i * 12..o + i * 12 + 12];
        let le = |s: &[u8]| s.iter().rev().fold(0u64, |a, &b| (a << 8) | b as u64);
        (le(&e[0..5]), le(&e[5..8]) as u32, le(&e[8..11]) as u32, e[11]) }).collect();
    let blocks_off = o; o += nblk * 12;
    let (methods_off, methods_len) = (o, ncm * cml); o += methods_len;
    if flags & 4 != 0 { let hs = u32at(o); o += 4 + hs * 2 + nblk * 20; }
    let (dir_off, dir_len) = (o, dirsz); o += dirsz;
    let _ = blocks_off;
    Ok(Toc { raw, header_size: hdr, ver, flags, cbs, ids, offlen, blocks, methods_off, methods_len, dir_off, dir_len, meta_off: o })
}

/// Write a container unencrypted. Unchanged chunks keep their original (decrypted) blocks; changed
/// chunks are Oodle-compressed with our own encoder when the container already uses Oodle, and
/// stored uncompressed otherwise. Only offsets, lengths, the block list and the per-chunk meta
/// change; ids and hash seeds are kept.
pub fn write_container(toc: &Toc, chunks: &[Vec<u8>], changed: &[bool],
                       read: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, String>,
                       decompress: &mut dyn FnMut(&[u8], usize) -> Result<Vec<u8>, String>) -> Result<(Vec<u8>, Vec<u8>), String> {
    let cbs = toc.cbs as u64;
    let mut ucas = Vec::new();
    let mut blocks: Vec<(u64, u32, u32, u8)> = Vec::new();
    let mut offlen: Vec<(u64, u64)> = Vec::new();
    let mut meta = Vec::new();
    let names = &toc.raw[toc.methods_off..toc.methods_off + toc.methods_len];
    let name_len = u32::from_le_bytes(toc.raw[40..44].try_into().unwrap()) as usize;
    let oodle = if name_len == 0 { None } else { names.chunks(name_len).position(|m| m.starts_with(b"Oodle\0")).map(|i| i as u8 + 1) };
    for (i, c) in chunks.iter().enumerate() {
        let voff = blocks.len() as u64 * cbs;
        offlen.push((voff, c.len() as u64));
        let (off, len) = toc.offlen[i];
        if !changed[i] && len > 0 && off % cbs == 0 {
            for bi in (off / cbs) as usize..=((off + len - 1) / cbs) as usize {
                let (raw, usz, m) = read_block(toc, bi, read)?;
                while ucas.len() % 16 != 0 { ucas.push(0); }
                blocks.push((ucas.len() as u64, raw.len() as u32, usz, m));
                ucas.extend_from_slice(&raw);
            }
        } else {
            for part in c.chunks(toc.cbs) {
                while ucas.len() % 16 != 0 { ucas.push(0); }
                // Only keep a compressed block that decompresses back to the input.
                let packed = oodle.and_then(|m| oodle::compress(part).map(|z| (m, z)))
                    .filter(|(_, z)| decompress(z, part.len()).is_ok_and(|d| d == part));
                if let Some((m, z)) = packed {
                    blocks.push((ucas.len() as u64, z.len() as u32, part.len() as u32, m));
                    ucas.extend_from_slice(&z);
                } else {
                    blocks.push((ucas.len() as u64, part.len() as u32, part.len() as u32, 0));
                    ucas.extend_from_slice(part);
                }
            }
        }
        let old_meta = &toc.raw[toc.meta_off + i * 33..toc.meta_off + i * 33 + 33];
        let mut h = [0u8; 32]; h[..20].copy_from_slice(&Sha1::digest(c));
        meta.extend_from_slice(&h); meta.push(old_meta[32] & !1);
    }
    let r = &toc.raw; let n = chunks.len();
    let mut out = Vec::new();
    out.extend_from_slice(&r[..toc.header_size]);
    out[80] &= !2; // written back decrypted
    out[28..32].copy_from_slice(&(blocks.len() as u32).to_le_bytes());
    // chunk ids, then offset/length (5+5 bytes big endian), then everything up to the block list unchanged
    let ids_off = toc.header_size;
    out.extend_from_slice(&r[ids_off..ids_off + n * 12]);
    for (o, l) in &offlen { out.extend_from_slice(&o.to_be_bytes()[3..]); out.extend_from_slice(&l.to_be_bytes()[3..]); }
    let mut p = ids_off + n * 22;
    let nseed = u32::from_le_bytes(r[84..88].try_into().unwrap()) as usize; let nnoph = u32::from_le_bytes(r[96..100].try_into().unwrap()) as usize;
    let extra = (if toc.ver >= 4 { nseed * 4 } else { 0 }) + (if toc.ver >= 5 { nnoph * 4 } else { 0 });
    out.extend_from_slice(&r[p..p + extra]); p += extra;
    for (boff, csz, usz, m) in &blocks {
        out.extend_from_slice(&boff.to_le_bytes()[..5]); out.extend_from_slice(&csz.to_le_bytes()[..3]); out.extend_from_slice(&usz.to_le_bytes()[..3]); out.push(*m);
    }
    let _ = p;
    out.extend_from_slice(&r[toc.methods_off..toc.methods_off + toc.methods_len]);
    if toc.flags & 4 != 0 { return Err("signed containers are not handled".into()); }
    let mut dir = r[toc.dir_off..toc.dir_off + toc.dir_len].to_vec();
    // Some packers encrypt the directory index along with the data, others leave it plain.
    // It starts with the mount point FString, so keep whichever copy reads as one.
    if toc.flags & 2 != 0 && dir.len() % 16 == 0 && !starts_with_fstring(&dir) {
        decrypt(&mut dir);
        if !starts_with_fstring(&dir) { return Err("can't read the directory index".into()); }
    }
    out.extend_from_slice(&dir);
    out.extend_from_slice(&meta);
    Ok((out, ucas))
}

fn starts_with_fstring(b: &[u8]) -> bool {
    let n = match b.get(..4) { Some(s) => i32::from_le_bytes(s.try_into().unwrap()), None => return false };
    n > 0 && (n as usize) < b.len() - 4 && b[3 + n as usize] == 0
}

/// One block as stored, decrypted: (bytes, uncompressed size, compression method).
fn read_block(toc: &Toc, bi: usize, read: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, String>) -> Result<(Vec<u8>, u32, u8), String> {
    let (boff, csz, usz, m) = *toc.blocks.get(bi).ok_or("block index out of range")?;
    let mut buf = if toc.flags & 2 != 0 {
        let mut b = read(boff, (csz as usize + 15) & !15)?; decrypt(&mut b); b
    } else { read(boff, csz as usize)? };
    buf.truncate(csz as usize);
    Ok((buf, usz, m))
}

/// One chunk of a container, decompressed. `read(offset, len)` supplies raw bytes from the .ucas,
/// `decompress(block, raw_len)` inflates an Oodle block.
pub fn read_chunk(toc: &Toc, ci: usize, read: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, String>,
                  decompress: &mut dyn FnMut(&[u8], usize) -> Result<Vec<u8>, String>) -> Result<Vec<u8>, String> {
    let (off, len) = toc.offlen[ci];
    if len == 0 { return Ok(Vec::new()); }
    let (first, last) = ((off / toc.cbs as u64) as usize, ((off + len - 1) / toc.cbs as u64) as usize);
    let mut out = Vec::with_capacity(len as usize + toc.cbs);
    for bi in first..=last {
        let (buf, usz, m) = read_block(toc, bi, read)?;
        if m == 0 { out.extend_from_slice(&buf[..usz as usize]); } else { out.extend(decompress(&buf, usz as usize)?); }
    }
    let s = (off % toc.cbs as u64) as usize;
    Ok(out[s..s + len as usize].to_vec())
}

pub struct ModResult { pub materials: usize, pub fixed: usize, pub problems: Vec<String>, pub utoc: Vec<u8>, pub ucas: Vec<u8> }

/// Check or fix one mod container held in memory. Returns the rewritten files when something was
/// fixed, and for every readable encrypted container so it ends up decrypted.
pub fn process_mod(toc: &Toc, ucas: &[u8], assume_old: bool, check_only: bool,
                   decompress: &mut dyn FnMut(&[u8], usize) -> Result<Vec<u8>, String>) -> Result<ModResult, String> {
    let (old, new) = load_schemas();
    let mut read = |o: u64, n: usize| -> Result<Vec<u8>, String> { ucas.get(o as usize..o as usize + n).map(|s| s.to_vec()).ok_or("ucas shorter than its TOC says".into()) };
    let encrypted = toc.flags & 2 != 0;
    if encrypted {
        // AES has no integrity check, so make sure the container header decrypts to its signature.
        let ci = toc.ids.iter().position(|id| id[11] == 6).ok_or("encrypted container without a container header")?;
        let header = read_chunk(toc, ci, &mut read, decompress)?;
        if !header.starts_with(b"nCoI") { return Err("encrypted with a key other than the game's".into()); }
    }
    let mut chunks = Vec::with_capacity(toc.ids.len());
    let mut changed = vec![false; toc.ids.len()];
    let (mut fixed, mut materials, mut problems) = (0usize, 0usize, Vec::new());
    for ci in 0..toc.ids.len() {
        let data = read_chunk(toc, ci, &mut read, decompress)?;
        if toc.ids[ci][11] == 1 {
            match fix_package(&data, &old, &new, assume_old) {
                Outcome::Fixed(k, newdata) => { fixed += k; materials += k; chunks.push(newdata); changed[ci] = true; continue; }
                Outcome::UpToDate(k) => materials += k,
                Outcome::Skipped(why) => problems.push(why),
                Outcome::NoMaterials => {}
            }
        }
        chunks.push(data);
    }
    if (fixed == 0 && !encrypted) || check_only || !problems.is_empty() { return Ok(ModResult { materials, fixed, problems, utoc: Vec::new(), ucas: Vec::new() }); }
    let (u, c) = write_container(toc, &chunks, &changed, &mut read, decompress)?;
    Ok(ModResult { materials, fixed, problems, utoc: u, ucas: c })
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    static mut KEEP: Vec<Vec<u8>> = Vec::new();   // result buffers stay alive until the next call
    static mut RESULT: [u32; 10] = [0; 10];

    #[no_mangle]
    pub extern "C" fn alloc(n: u32) -> *mut u8 {
        let mut v = Vec::with_capacity(n as usize); let p = v.as_mut_ptr(); std::mem::forget(v); p
    }
    #[no_mangle]
    pub extern "C" fn dealloc(p: *mut u8, n: u32) {
        unsafe { drop(Vec::from_raw_parts(p, 0, n as usize)); }
    }
    fn keep(v: Vec<u8>) -> (u32, u32) { let r = (v.as_ptr() as u32, v.len() as u32); unsafe { (*std::ptr::addr_of_mut!(KEEP)).push(v); } r }

    /// Result layout (u32 each): status (0 ok, 1 error), msg ptr, msg len, materials, fixed,
    /// utoc ptr, utoc len, ucas ptr, ucas len, problems count.
    #[no_mangle]
    pub extern "C" fn fix_mod(utoc: *const u8, utoc_len: u32, ucas: *const u8, ucas_len: u32, assume_old: u32, check_only: u32) -> *const u32 {
        unsafe { (*std::ptr::addr_of_mut!(KEEP)).clear(); }
        let utoc_v = unsafe { std::slice::from_raw_parts(utoc, utoc_len as usize) }.to_vec();
        let ucas_s = unsafe { std::slice::from_raw_parts(ucas, ucas_len as usize) };
        let mut dec = |block: &[u8], raw_len: usize| -> Result<Vec<u8>, String> {
            let mut out = vec![0u8; raw_len];
            let n = oozextract::Extractor::new().read_from_slice(block, &mut out).map_err(|e| format!("Oodle block: {e}"))?;
            if n != raw_len { return Err("Oodle block: short output".into()); }
            Ok(out)
        };
        let res = parse_toc(utoc_v).and_then(|toc| process_mod(&toc, ucas_s, assume_old != 0, check_only != 0, &mut dec));
        let r = unsafe { &mut *std::ptr::addr_of_mut!(RESULT) };
        *r = [0; 10];
        match res {
            Err(e) => { let (p, l) = keep(e.into_bytes()); r[0] = 1; r[1] = p; r[2] = l; }
            Ok(m) => {
                let msg = m.problems.first().cloned().unwrap_or_default();
                let (p, l) = keep(msg.into_bytes()); r[1] = p; r[2] = l; r[3] = m.materials as u32; r[4] = m.fixed as u32; r[9] = m.problems.len() as u32;
                if !m.utoc.is_empty() { let (a, b) = keep(m.utoc); let (c, d) = keep(m.ucas); r[5] = a; r[6] = b; r[7] = c; r[8] = d; }
            }
        }
        r.as_ptr()
    }
}
