#!/usr/bin/env bash
# Assembles the boxb.org site into web/dist. The Ruffle player comes from
# ./build-ruffle.sh (web/dist/ruffle), which only needs re-running when Ruffle
# or the vendored swf change.
set -euo pipefail
cd "$(dirname "$0")"

[ -f dist/ruffle/ruffle.js ] || ./build-ruffle.sh

# Patcher (Rust -> wasm)
cargo build -p kalich-web-patcher --target wasm32-unknown-unknown --release
rm -rf dist/lib/patcher
wasm-bindgen --target web --out-dir dist/lib/patcher --no-typescript \
  ../target/wasm32-unknown-unknown/release/kalich_web_patcher.wasm
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int \
    -o dist/lib/patcher/kalich_web_patcher_bg.wasm dist/lib/patcher/kalich_web_patcher_bg.wasm
fi

# JS libraries
npm ci --no-audit --no-fund
npx esbuild src/music.js --bundle --format=esm --minify --outfile=dist/lib/music.js
cp node_modules/spessasynth_lib/dist/spessasynth_processor.min.js dist/lib/
rm -rf dist/lib/libarchive && mkdir -p dist/lib/libarchive
cp node_modules/libarchive.js/dist/libarchive.js node_modules/libarchive.js/dist/worker-bundle.js \
   node_modules/libarchive.js/dist/libarchive.wasm dist/lib/libarchive/

# Pages
cp -R site/. dist/
echo "Site in web/dist"
