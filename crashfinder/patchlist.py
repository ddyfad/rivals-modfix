#!/usr/bin/env python3
"""Write patch-files.txt for the crash finder page: every package path in a game patch
container, lowercased, one per line, gzip'd and base64'd (the artifact host only serves text). Run after each game update:
  ./patchlist.py "$PAKS/Patch_-Windows_<build>_P.utoc"
The index of an encrypted container is decrypted with the game key below."""
import base64, gzip, struct, sys
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

KEY = bytes.fromhex('0C263D8C22DCB085894899C3A3796383E9BF9DE0CBFB08C9BF2DEF2E84F29D74')

def directory_index(b):
    ver = b[16]; hdr = struct.unpack_from('<I', b, 20)[0]
    n, nblk, blksz, ncm, cml, _, dirsz, _ = struct.unpack_from('<8I', b, 24)
    flags = b[80]; nseed = struct.unpack_from('<I', b, 84)[0]; nnoph = struct.unpack_from('<I', b, 96)[0]
    o = hdr + n * 12 + n * 10
    if ver >= 4: o += nseed * 4
    if ver >= 5: o += nnoph * 4
    o += nblk * blksz + ncm * cml
    if flags & 4:
        hs = struct.unpack_from('<I', b, o)[0]; o += 4 + hs * 2 + nblk * 20
    idx = b[o:o + dirsz]
    if flags & 2:
        d = Cipher(algorithms.AES(KEY), modes.ECB()).decryptor(); idx = d.update(idx) + d.finalize()
    return idx

def paths(idx):
    p = 0
    def fstr():
        nonlocal p
        ln = struct.unpack_from('<i', idx, p)[0]; p += 4
        if ln < 0: s = idx[p:p - ln * 2].decode('utf-16le'); p += -ln * 2
        else: s = idx[p:p + ln].decode('latin-1'); p += ln
        return s.rstrip('\0')
    mount = fstr()
    c = struct.unpack_from('<I', idx, p)[0]; p += 4
    dirs = [struct.unpack_from('<4I', idx, p + i * 16) for i in range(c)]; p += c * 16
    c = struct.unpack_from('<I', idx, p)[0]; p += 4
    files = [struct.unpack_from('<3I', idx, p + i * 12) for i in range(c)]; p += c * 12
    c = struct.unpack_from('<I', idx, p)[0]; p += 4
    strs = [fstr() for _ in range(c)]
    NONE = 0xffffffff
    out = []
    def walk(d, path):
        name, child, sib, f = dirs[d]
        here = path + (strs[name] + '/' if name != NONE else '')
        while f != NONE:
            fn, nxt, _ = files[f]; out.append(here + strs[fn]); f = nxt
        while child != NONE:
            walk(child, here); child = dirs[child][2]
    walk(0, mount)
    return out

def stem(path):
    path = path.lower().lstrip('./')
    return path.rsplit('.', 1)[0] if '.' in path.rsplit('/', 1)[-1] else path

if __name__ == '__main__':
    stems = sorted({stem(x) for x in paths(directory_index(open(sys.argv[1], 'rb').read()))})
    gz = gzip.compress('\n'.join(stems).encode(), 9, mtime=0)
    open('patch-files.txt', 'w').write(base64.b64encode(gz).decode())
    print(len(stems), 'packages')
