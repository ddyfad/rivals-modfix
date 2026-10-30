import struct, sys
def parse(path, nlen=2, elen=2):
    b = open(path, 'rb').read(); p = 0
    def u8():
        nonlocal p; v = b[p]; p += 1; return v
    def u16():
        nonlocal p; v = struct.unpack_from('<H', b, p)[0]; p += 2; return v
    def u32():
        nonlocal p; v = struct.unpack_from('<I', b, p)[0]; p += 4; return v
    n = u32(); names = []
    for _ in range(n):
        ln = u16() if nlen == 2 else u8(); names.append(b[p:p+ln].decode('utf-8', 'replace')); p += ln
    nm = lambda i: names[i] if i != 0xffffffff else None
    enums = {}
    for _ in range(u32()):
        en = nm(u32()); c = u16() if elen == 2 else u8(); vals = []
        for _ in range(c):
            v = struct.unpack_from('<Q', b, p)[0]; p += 8; vals.append((v, nm(u32())))
        enums[en] = vals
    def ptype():
        t = u8()
        if t == 26: return ('Enum', ptype(), nm(u32()))
        if t == 9: return ('Struct', nm(u32()))
        if t in (8, 25, 28): return ({8:'Array',25:'Set',28:'Optional'}[t], ptype())
        if t == 24: return ('Map', ptype(), ptype())
        return (t,)
    structs = {}
    for _ in range(u32()):
        sn = nm(u32()); sup = nm(u32()); pc = u16(); spc = u16(); props = []
        for _ in range(spc):
            idx = u16(); asz = u8(); pn = nm(u32()); props.append((idx, pn, asz, ptype()))
        structs[sn] = (sup, pc, props)
    assert p == len(b), (p, len(b))
    return names, enums, structs
def parse_any(path):
    for nlen in (1, 2):
        for elen in (1, 2):
            try: return parse(path, nlen, elen)
            except Exception as e: err = e
    raise err
if __name__ == '__main__':
    a, b_ = parse_any(sys.argv[1]), parse_any(sys.argv[2])
    print('names', len(a[0]), len(b_[0]), 'enums', len(a[1]), len(b_[1]), 'structs', len(a[2]), len(b_[2]))
    sa, sb = a[2], b_[2]
    added = sorted(set(sb) - set(sa)); removed = sorted(set(sa) - set(sb))
    print('structs added', len(added), added[:40]); print('structs removed', len(removed), removed[:40])
    changed = []
    for k in sorted(set(sa) & set(sb)):
        if sa[k] != sb[k]: changed.append(k)
    print('structs changed', len(changed))
    for k in changed:
        (s1, c1, p1), (s2, c2, p2) = sa[k], sb[k]
        d1 = {x[1]: x for x in p1}; d2 = {x[1]: x for x in p2}
        add = [x for x in d2 if x not in d1]; rem = [x for x in d1 if x not in d2]
        mv = [x for x in d1 if x in d2 and d1[x] != d2[x]]
        print(f'  {k} (super {s1}->{s2}, props {c1}->{c2}): +{add[:6]} -{rem[:6]} changed {len(mv)}' + (f' e.g. {mv[:3]}' if mv else ''))
    ea, eb = a[1], b_[1]
    ech = [k for k in set(ea) & set(eb) if ea[k] != eb[k]]
    print('enums added', len(set(eb)-set(ea)), 'removed', len(set(ea)-set(eb)), 'changed', len(ech), sorted(ech)[:20])
