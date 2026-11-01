# ModMend

https://mrm.dafyddjon.es

Fixes Marvel Rivals mods that started crashing the game after an update.

The Oct 30 2026 update changed how the game stores materials, so any skin mod built before it
crashes the game a few seconds after launch. ModMend finds those mods and rebuilds them to work
with the new version.

## How to use it

1. Open https://mrm.dafyddjon.es.
2. Pick your `~mods` folder (or drop it on the page). The page shows where to find it.
3. It lists each mod as fine or broken. Download the fixed copies, one at a time or all at once.
4. Unzip them into `~mods`, replacing the old files. Keep your old copies until the game runs.

Everything happens in your browser. Your mods are never uploaded anywhere.

## What's in this repo

- `crashfinder/page.html`: the website.
- `modfix/rivals-modfix/`: the code that reads and repairs the mod files. The website runs it as
  WebAssembly. Its README has the technical details and build steps.
