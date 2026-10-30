#!/usr/bin/env python3
"""Port material instances cooked for one Marvel Rivals build to the next.

The Oct 30 2026 build (3984769) inserted two bools into FMaterialInstanceBasePropertyOverrides
at schema index 40. Every MI_ cooked before then is read with the wrong unversioned-property
layout and the async loader crashes. This rewrites the unversioned header of that struct inside
each MaterialInstanceConstant export so the old data lines up with the new layout.

  fixmi.py fix <dir from retoc unpack> [--dry]       patch .uasset/.uexp in place (legacy format)
  fixmi.py walk <zen .uasset> <usmap>                 debug: walk a raw zen package's first export
"""
import os, struct, subprocess, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import usmap as usmaplib

HERE = os.path.dirname(os.path.abspath(__file__))
OLD_USMAP, NEW_USMAP = f'{HERE}/Marvel-3962284.usmap', f'{HERE}/Marvel-3984769.usmap'
STRUCT, INSERT_AT, INSERT_N = 'MaterialInstanceBasePropertyOverrides', 40, 2

def load_usmap(path):
    hdr = open(path, 'rb').read(16)
    raw = open(path, 'rb').read()[16:]
    if hdr[7] == 3: raw = subprocess.run(['zstd', '-d', '-c'], input=raw, capture_output=True, check=True).stdout
    elif hdr[7] != 0: raise SystemExit(f'{path}: unsupported usmap compression {hdr[7]}')
    tmp = path + '.raw'; open(tmp, 'wb').write(raw)
    try: return usmaplib.parse_any(tmp)
    finally: os.unlink(tmp)

NATIVE = {'Guid': 16, 'LinearColor': 16, 'Color': 4, 'Vector': 24, 'Vector3f': 12, 'Vector2D': 16, 'Vector2f': 8,
          'Vector4': 32, 'Vector4f': 16, 'IntPoint': 8, 'IntVector': 12, 'IntVector4': 16, 'Rotator': 24, 'Quat': 32,
          'Quat4f': 16, 'Transform': 80, 'Matrix': 128, 'Box': 49, 'Box2D': 33, 'BoxSphereBounds': 56, 'Plane': 32,
          'DateTime': 8, 'Timespan': 8, 'FrameNumber': 4, 'FrameRate': 8, 'PerPlatformFloat': 8, 'PerPlatformInt': 8,
          'PerPlatformBool': 5, 'GameplayTag': 8}
SIZES = {0: 1, 1: 1, 2: 4, 3: 4, 4: 4, 5: 8, 7: 8, 12: 4, 14: 4, 15: 16, 18: 8, 19: 4, 20: 2, 21: 8, 22: 2, 23: 1}

class Walker:
    def __init__(self, maps, data, names=None, soft_as_index=False):
        self.maps, self.b, self.names, self.soft_as_index = maps, data, names, soft_as_index
        self.marks = {}   # struct name -> list of (header offset, header end) seen while walking
    def chain(self, cls):
        # Schema order is the class itself first, then each super class in turn.
        out = []
        while cls:
            sup, pc, props = self.maps[2][cls]; out.append((cls, pc, props)); cls = sup
        base, slots = 0, {}
        for cls, pc, props in out:
            for idx, name, asz, t in props:
                for k in range(asz): slots[base + idx + k] = (name if asz == 1 else f'{name}[{k}]', t)
            base += pc
        return base, slots
    def fname(self, p):
        i, n = struct.unpack_from('<II', self.b, p)
        s = self.names[i] if self.names and i < len(self.names) else f'name#{i}'
        return s if n == 0 else f'{s}_{n-1}'
    def fstring(self, p):
        ln = struct.unpack_from('<i', self.b, p)[0]
        return p + 4 + (-ln * 2 if ln < 0 else ln)
    def header(self, p):
        frags = []
        while True:
            v = struct.unpack_from('<H', self.b, p)[0]; p += 2
            frags.append((v & 0x7f, bool(v & 0x80), v >> 9, bool(v & 0x100)))
            if v & 0x100: break
        nz = sum(f[2] for f in frags if f[1])
        mask = 0
        if nz:
            w = 1 if nz <= 8 else 2 if nz <= 16 else 4 * ((nz + 31) // 32)
            mask = int.from_bytes(self.b[p:p+w], 'little'); p += w
        return frags, mask, p
    def walk_struct(self, cls, p, out):
        """Walk unversioned properties of struct/class `cls` at offset p. Returns end offset."""
        nslots, slots = self.chain(cls)
        h0 = p; frags, mask, p = self.header(p)
        self.marks.setdefault(cls, []).append((h0, p))
        out.append(f'{cls} header @{h0}: {frags} mask={mask:#x}')
        idx = 0; zbit = 0
        for skip, hz, nval, last in frags:
            idx += skip
            for _ in range(nval):
                zero = False
                if hz: zero = bool(mask >> zbit & 1); zbit += 1
                if idx not in slots: raise ValueError(f'{cls}: schema slot {idx} is not serialisable')
                name, t = slots[idx]
                if not zero:
                    q = self.value(t, p, out, name)
                    out.append(f'{cls}.{name} @{p} +{q-p}'); p = q
                else: out.append(f'{cls}.{name} = 0')
                idx += 1
        return p
    def value(self, t, p, out, name=''):
        code = {'Struct': 9, 'Enum': 26, 'Array': 8, 'Set': 25, 'Optional': 28, 'Map': 24}.get(t[0], t[0])
        if code in SIZES: return p + SIZES[code]
        if code == 26: return self.value(t[1], p, out)
        if code in (10, 29, 30): return self.fstring(p)
        if code in (16, 17):
            return p + 4 if self.soft_as_index else self.fstring(p + 8)
        if code == 9:
            sn = t[1]
            if sn in NATIVE: return p + NATIVE[sn]
            if sn in ('SoftObjectPath', 'SoftClassPath'): return self.value((17,), p, out)
            if sn not in self.maps[2]: raise ValueError(f'unknown struct {sn}')
            return self.walk_struct(sn, p, out)
        if code in (8, 25):
            n = struct.unpack_from('<i', self.b, p)[0]; p += 4
            for _ in range(n): p = self.value(t[1], p, out)
            return p
        if code == 24:
            p += 4  # NumKeysToRemove
            n = struct.unpack_from('<i', self.b, p)[0]; p += 4
            for _ in range(n): p = self.value(t[1], p, out); p = self.value(t[2], p, out)
            return p
        if code == 6: return p + 12
        if code == 13:
            n = struct.unpack_from('<i', self.b, p)[0]; return p + 4 + 12 * n
        if code == 28: return self.value(t[1], p + 4, out) if struct.unpack_from('<i', self.b, p)[0] else p + 4
        raise ValueError(f'unsupported property type {t} for {name}')

def shift_header(b, h0, h1, at, n):
    """Insert `n` skipped slots at schema index `at` into the unversioned header at b[h0:h1]."""
    frags = []; p = h0
    while True:
        v = struct.unpack_from('<H', b, p)[0]; p += 2
        frags.append([v & 0x7f, bool(v & 0x80), v >> 9, bool(v & 0x100)])
        if v & 0x100: break
    maskbytes = b[p:h1]
    out, idx, done = [], 0, False
    for skip, hz, nval, last in frags:
        if not done and idx + skip >= at:          # insertion lands inside this skip run
            out.append([skip + n, hz, nval, last]); done = True
        elif not done and idx + skip + nval > at:  # insertion lands inside the value run: split it
            before = at - idx - skip
            out.append([skip, hz, before, False]); out.append([n, hz, nval - before, last]); done = True
        else: out.append([skip, hz, nval, last])
        idx += skip + nval
    if not done:  # nothing serialised at or after the insertion point
        return b[h0:h1]
    for f in out:
        if f[0] > 127 or f[2] > 127: raise ValueError('fragment overflow')
    hdr = b''.join(struct.pack('<H', f[0] | (0x80 if f[1] else 0) | (0x100 if f[3] else 0) | (f[2] << 9)) for f in out)
    return hdr + maskbytes

def read_legacy(ua):
    b = open(ua, 'rb').read(); p = 0
    def i32():
        nonlocal p; v = struct.unpack_from('<i', b, p)[0]; p += 4; return v
    def i64():
        nonlocal p; v = struct.unpack_from('<q', b, p)[0]; p += 8; return v
    def fstr():
        nonlocal p; ln = i32(); s = b[p:p+(-ln*2 if ln < 0 else ln)]; p += -ln*2 if ln < 0 else ln
        return s.decode('utf-16le' if ln < 0 else 'latin-1').rstrip('\0')
    assert struct.unpack_from('<I', b, 0)[0] == 0x9E2A83C1, 'not a uasset'; p = 4
    legacy = i32(); i32(); ue4 = i32(); ue5 = i32() if legacy <= -8 else 0; i32()
    cv = i32(); p += 20 * cv                     # custom versions
    total_header = i32(); fstr(); flags = struct.unpack_from('<I', b, p)[0]; p += 4
    name_count, name_off = i32(), i32()
    soft_count = soft_off = 0
    if ue5 >= 1008 or ue5 == 0: soft_count, soft_off = i32(), i32()   # 0 = unversioned, i.e. latest
    if not flags & 0x80000000: fstr()           # LocalizationId, uncooked only
    i32(); i32()                                 # gatherable text
    exp_count, exp_off, imp_count, imp_off = i32(), i32(), i32(), i32()
    depends_off = i32()
    names = []; q = name_off
    for _ in range(name_count):
        ln = struct.unpack_from('<i', b, q)[0]; q += 4
        s = b[q:q+(-ln*2 if ln < 0 else ln)]; q += (-ln*2 if ln < 0 else ln) + 4
        names.append(s.decode('utf-16le' if ln < 0 else 'latin-1').rstrip('\0'))
    imports = []
    for i in range(imp_count):
        q = imp_off + i * 32
        cp, _, cn, _ = struct.unpack_from('<4I', b, q)
        imports.append((names[cp], names[cn], names[struct.unpack_from('<I', b, q + 20)[0]]))
    esz = (depends_off - exp_off) // exp_count
    exports = []
    for i in range(exp_count):
        q = exp_off + i * esz
        cls = struct.unpack_from('<i', b, q)[0]; nm = names[struct.unpack_from('<I', b, q + 16)[0]]
        ssize, soff = struct.unpack_from('<qq', b, q + 28)
        exports.append(dict(i=i, entry=q, cls=imports[-cls - 1][2] if cls < 0 else f'export{cls}', name=nm, size=ssize, off=soff))
    return dict(bytes=b, names=names, total_header=total_header, exports=exports, soft=soft_count > 0, esz=esz)

def fix_dir(root, dry):
    old, new = load_usmap(OLD_USMAP), load_usmap(NEW_USMAP)
    npk = nfix = 0
    for dp, _, fs in os.walk(root):
        for f in fs:
            if not f.endswith('.uasset'): continue
            ua = os.path.join(dp, f); ux = ua[:-7] + '.uexp'
            if not os.path.exists(ux): continue
            pk = read_legacy(ua)
            mics = [e for e in pk['exports'] if e['cls'] in ('MaterialInstanceConstant', 'MaterialInstanceDynamic')]
            if not mics: continue
            npk += 1
            uexp = bytearray(open(ux, 'rb').read()); hdr = bytearray(pk['bytes']); delta_total = 0; touched = False
            for e in mics:
                start = e['off'] - pk['total_header'] + delta_total
                blob = bytes(uexp[start:start + e['size']])
                wo = Walker(old, blob, pk['names'], pk['soft']); log = []
                try: end_old = wo.walk_struct(e['cls'], 0, log)
                except Exception as ex:
                    print(f'SKIP {ua}: {e["name"]}: cannot walk with the old layout: {ex}'); continue
                marks = wo.marks.get(STRUCT, [])
                if not marks:
                    print(f'ok   {ua}: {e["name"]}: no {STRUCT} serialised'); continue
                h0, h1 = marks[0]
                newhdr = shift_header(blob, h0, h1, INSERT_AT, INSERT_N)
                patched = blob[:h0] + newhdr + blob[h1:]
                delta = len(patched) - len(blob)
                wn = Walker(new, patched, pk['names'], pk['soft']); log2 = []
                end_new = wn.walk_struct(e['cls'], 0, log2)
                vals_old = [l.split(' @')[0] for l in log]; vals_new = [l.split(' @')[0] for l in log2]
                if end_new != end_old + delta or vals_old != vals_new or patched[end_new:] != blob[end_old:]:
                    print(f'FAIL {ua}: {e["name"]}: re-walk mismatch'); continue
                if dry: print(f'would fix {ua}: {e["name"]} (+{delta} bytes)'); continue
                uexp[start:start + e['size']] = patched
                struct.pack_into('<q', hdr, e['entry'] + 28, e['size'] + delta)
                for o in pk['exports']:
                    if o['off'] > e['off']: struct.pack_into('<q', hdr, o['entry'] + 36, o['off'] + delta)
                e['size'] += delta; delta_total += delta; nfix += 1; touched = True
                print(f'fixed {ua}: {e["name"]} (+{delta} bytes)')
            if touched:
                total = pk['total_header'] + len(uexp) - delta_total
                # BulkDataStartOffset == old uasset+uexp size; bump it along with the data
                if delta_total:
                    for q in range(len(hdr) - 8):
                        if struct.unpack_from('<q', hdr, q)[0] == total:
                            struct.pack_into('<q', hdr, q, total + delta_total); break
                open(ux, 'wb').write(uexp); open(ua, 'wb').write(hdr)
    print(f'{npk} material instance packages, {nfix} exports fixed')

def read_zen(path):
    b = open(path, 'rb').read()
    has_ver, hsize = struct.unpack_from('<II', b, 0)
    exp_map, bundle = struct.unpack_from('<ii', b, 32)
    p = 52 if not has_ver else None
    if p is None: raise SystemExit('versioned zen package not handled')
    cnt, nbytes, _ = struct.unpack_from('<IIQ', b, p); p += 16 + 8 * cnt
    hdrs = [struct.unpack_from('>H', b, p + 2 * i)[0] for i in range(cnt)]; p += 2 * cnt
    names = []
    for h in hdrs:
        ln = h & 0x7fff; w = bool(h & 0x8000)
        names.append(b[p:p + (ln * 2 if w else ln)].decode('utf-16le' if w else 'latin-1')); p += ln * 2 if w else ln
    exports = []
    for i in range((bundle - exp_map) // 72):
        q = exp_map + i * 72; off, size = struct.unpack_from('<QQ', b, q); ni = struct.unpack_from('<I', b, q + 16)[0] & 0x3fffffff
        exports.append(dict(name=names[ni], off=hsize + off, size=size))
    return b, names, exports

def walk_zen(path, usmap_path, cls='MaterialInstanceConstant'):
    maps = load_usmap(usmap_path); b, names, exports = read_zen(path)
    e = exports[0]; blob = b[e['off']:e['off'] + e['size']]
    w = Walker(maps, blob, names, soft_as_index=False); log = []
    try: end = w.walk_struct(cls, 0, log)
    except Exception as ex: print('ERROR', ex)
    else: print(f'{e["name"]}: properties end at {end} of {e["size"]}')
    for l in log: print(' ', l)

if __name__ == '__main__':
    if len(sys.argv) >= 3 and sys.argv[1] == 'fix': fix_dir(sys.argv[2], '--dry' in sys.argv)
    elif len(sys.argv) == 4 and sys.argv[1] == 'walk': walk_zen(sys.argv[2], sys.argv[3])
    else: print(__doc__)
