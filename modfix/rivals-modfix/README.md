# rivals-modfix

One-file Windows tool for players. Finds mods in `~mods` whose material instances were built
before the Oct 30 2026 update and ports them to the new layout, so the game stops crashing a
few seconds after launch.

## For players

1. Download `rivals-modfix.exe` and double-click it. It finds the game's `~mods` folder through
   Steam on its own. If it can't, drag the `~mods` folder onto the exe, or run it from a
   command prompt with the folder as the argument.
2. The first run downloads `oo2core_9_win64.dll` (the game's compression library, needed to read
   the mods) next to the exe. If your machine blocks that, put the DLL there yourself.
3. Read the list. `FIXED` mods were rewritten, `ok` mods were fine, `SKIP` mods were left alone
   with the reason printed. Originals of every rewritten mod are copied to
   `Paks\modfix-backup\` first. Delete that folder once the game runs.

Run it with `--check` to only report, without changing anything. Encrypted packs (RivalsFPS
and others packed with the game's public AES key) are decrypted and written back unencrypted.

## What it does

For every `.utoc` in `~mods` it reads the container, and in every package finds the
`MaterialInstanceConstant` exports. Each one is parsed with both the old and the new property
layout; a parse only counts when every property reads and the field after them is zero, which a
wrong layout almost never gives. Materials that read only with the old layout get a two-slot skip
inserted into their `BasePropertyOverrides` header at index 40, are re-read with the new layout,
and are only accepted if every value and the end position agree. The container is written back
with all chunks stored uncompressed (the game reads those fine), the per-chunk hashes
recomputed, and everything else in the TOC untouched.

Materials whose override values are only bools and enums read cleanly both ways; those are
treated as old if the mod file is dated before the update.

## Building

```
X=~/.cache/xwin
CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=lld-link \
CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-Lnative=$X/crt/lib/x86_64 -Lnative=$X/sdk/lib/um/x86_64 -Lnative=$X/sdk/lib/ucrt/x86_64" \
cargo build --release --target x86_64-pc-windows-msvc
```

Cross-compiles from Linux with `lld-link` and the xwin CRT/SDK.

The crash finder page runs the same code as WebAssembly (`oozextract` replaces the Oodle DLL):

```
cargo build --release --lib --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/rivals_modfix.wasm ../../crashfinder/modfix.wasm
```

then republish the page with `modfix.wasm` as a supporting file. The page pairs each dropped
`.utoc` with its `.ucas`, calls `fix_mod` to check, and on "Fix and download" calls it again
and hands back a zip of the rewritten container (the download allowlist has no .utoc/.ucas). `src/schema.json` is the
subset of the game's property mappings the walker needs, generated from the two usmaps in the
parent folder (`../fixmi.py` has the same logic in Python). A native Linux build works too and
uses `~/.cargo/bin/liboo2corelinux64.so.9`.

Tested under wine on `Hela_GhostBlade_19999999_P`: CUE4Parse reads the rewritten container
under the 3984769 usmap to the same values as the original under the 3962284 usmap, and the
patched override header is byte-identical to the game's own Oct 30 cook of the same material.
