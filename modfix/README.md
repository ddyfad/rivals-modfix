# modfix

Ports Marvel Rivals mods across the Oct 30 2026 update (build 3962284 -> 3984769).

## What broke

The Oct 30 build added two bools to `FMaterialInstanceBasePropertyOverrides`
(`bOverride_RenderExtraDepthWithoutMasked`, `bRenderExtraDepthWithoutMasked`) at schema
index 40, in the middle of the struct. Every material instance serialises that struct with
unversioned properties, which are addressed by index, so every `MI_` cooked before the update
is now read with the wrong layout: `BlendMode`, `ShadingModel`, `OpacityMaskClipValue` and
the rest shift by two, the stream goes out of step, and the async loader writes through a
garbage pointer. That is the `FAsyncLoadingThread` access violation people see a few seconds
after launch. Nothing else that mods normally touch changed (textures, meshes, widgets are
fine); `AnimBlueprintGeneratedConstantData` also changed, which only matters for mods that
ship animation blueprints.

Evidence: diff of the two usmaps (`usmap.py`), and the game's own Oct 30 cook of
`MI_1024001_Body` has the override header `14040b02010686020602020501` where the Oct 22 cook
has `...0486...` (skip 4 -> skip 6 at the insertion point).

## Fix a mod

```
retoc-rivals-cli unpack Mod_P.utoc -o work/Mod_P --game-paks-dir "$PAKS"
./fixmi.py fix work/Mod_P            # add --dry to only list what would change
retoc-rivals-cli pack work/Mod_P -o out --no-mod-suffix
```

`fix` finds every `MaterialInstanceConstant` export, walks its properties with the old
layout, inserts a two-slot skip into the overrides header, re-walks with the new layout and
only writes the file if both walks agree on every value and on where the properties end.
Packages without material instances are left alone. RivalsFPS packs are encrypted and are
rebuilt by RivalsFPS anyway.

Verified on `Hela_GhostBlade_19999999_P`: after the fix its `MI_1024001_Body` override
header is byte-identical to the game's Oct 30 cook, and CUE4Parse reads the repacked
container with the 3984769 usmap to exactly the values it read from the original with the
3962284 usmap.

## Files

- `fixmi.py`: the patcher. `walk <zen .uasset> <usmap>` prints the property walk of a raw
  IoStore package, for checking a layout against the game's own files.
- `usmap.py`: usmap reader and differ (`usmap.py old.raw new.raw`, files decompressed with
  `tail -c +17 x.usmap | zstd -d`).
- `Marvel-3962284.usmap`, `Marvel-3984769.usmap`: from SpaceDepot/rivals-depot.
