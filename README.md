# ModMend

https://mrm.dafyddjon.es

Fixes Marvel Rivals mods that started crashing the game after an update.

## How to use it

1. Open https://mrm.dafyddjon.es.
2. Pick your `~mods` folder (or drop it on the page). The page shows where to find it.
3. It lists each mod as fine or broken. Download the fixed copies, one at a time or all at once.
4. Unzip them into `~mods`, replacing the old files. Keep your old copies until the game runs.

Everything happens in your browser. Your mods are never uploaded anywhere.

## What does it patch?

Skin mods usually come with their own material files, which tell the game how a texture should
look (colours, glow, shine and so on). Each material stores a list of settings in a fixed order,
and the game reads them back in that same order.

The Oct 30 2026 update added two new settings near the middle of that list. Mods made before the
update don't have them, so from that point on the game reads every setting one slot off. The
values it gets are nonsense, and it crashes a few seconds after launch, usually as the mod's
skin loads.

ModMend opens each mod, finds the materials still using the old layout, and adds the two missing
settings in the right place so everything lines up again. Each fixed material is read back with
the new layout and only kept if every value matches the original, so it won't change how the skin
looks. Mods that are already fine are left alone.

## What's in this repo

- `crashfinder/page.html`: the website.
- `modfix/rivals-modfix/`: the code that reads and repairs the mod files. The website runs it as
  WebAssembly. Its README has the technical details and build steps.
