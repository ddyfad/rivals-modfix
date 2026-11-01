# rivals-modfix

The code behind the Rivals Mod Fixer page (https://mrm.dafyddjon.es). It finds mods in `~mods`
whose material instances were built before the Oct 30 2026 update and ports them to the new
layout, so the game stops crashing a few seconds after launch. It runs in the browser as
WebAssembly; nothing is uploaded. The page itself is `../../crashfinder/page.html`.

## What it does

For every `.utoc` in `~mods` it reads the container, and in every package finds the
`MaterialInstanceConstant` exports. Each one is parsed with both the old and the new property
layout; a parse only counts when every property reads and the field after them is zero, which a
wrong layout almost never gives. Materials that read only with the old layout get a two-slot skip
inserted into their `BasePropertyOverrides` header at index 40, are re-read with the new layout,
and are only accepted if every value and the end position agree. The container is written back,
encrypted again if it was encrypted: unchanged chunks keep their original compressed blocks, patched packages are
compressed with `src/oodle.rs` (a small open-source Oodle encoder, Mermaid format with
Huffman-coded streams) when the container already uses Oodle and stored uncompressed otherwise. Every block
it writes is decompressed and compared before it's used. The per-chunk hashes are recomputed
and everything else in the TOC is untouched.

Materials whose override values are only bools and enums read cleanly both ways; those are
treated as old if the mod file is dated before the update.

## Building

The page runs this crate as WebAssembly, with `oozextract` for Oodle decompression:

```
cargo build --release --lib --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/rivals_modfix.wasm ../../crashfinder/modfix.wasm
```

then republish the page with `modfix.wasm` as a supporting file. The page pairs each dropped
`.utoc` with its `.ucas`, calls `fix_mod` to check, and on "Fix and download" calls it again
and hands back a zip of the rewritten container (the download allowlist has no .utoc/.ucas). `src/schema.json` is the
subset of the game's property mappings the walker needs, generated from the build-matched usmaps in
SpaceDepot/rivals-depot.

Tested under wine on `Hela_GhostBlade_19999999_P`: CUE4Parse reads the rewritten container
under the 3984769 usmap to the same values as the original under the 3962284 usmap, and the
patched override header is byte-identical to the game's own Oct 30 cook of the same material.
