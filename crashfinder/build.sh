#!/bin/sh
# Builds dist/ for the self-hosted copy (mrm.dafyddjon.es, /opt/crash/site on dj). The claude.ai
# artifact is published from page.html directly; the host wraps it in the document skeleton itself.
set -e
cd "$(dirname "$0")"
mkdir -p dist
{
  printf '<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n<meta name="viewport" content="width=device-width, initial-scale=1">\n<meta name="description" content="Fix Marvel Rivals mods broken by the Oct 30 update. Pick your ~mods folder and download fixed copies.">\n'
  sed -n '/^<title>/,/^<\/style>$/p' page.html
  printf '</head>\n<body>\n'
  sed '1,/^<\/style>$/d' page.html
  printf '</body>\n</html>\n'
} > dist/index.html
cp modfix.wasm jszip.min.js dist/
echo "dist/: $(ls dist | tr '\n' ' ')"
