#!/usr/bin/env bash
# Builds the browser player bundle into web/dist: player.js (mount API),
# sw.js, the WebAssembly patcher, libraries and the Ruffle build. The boxb.org
# site serves it at /zlaty-kalich/. Ruffle comes from ./build-ruffle.sh, which
# only needs re-running when Ruffle or the vendored swf change.
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

# Player bundle (idb, SpessaSynth and libarchive.js bundled in; the patcher
# and Ruffle load lazily from lib/ and ruffle/).
npm ci --no-audit --no-fund
npx esbuild src/player.js --bundle --format=esm --minify --outfile=dist/player.js
npx esbuild src/sw.js --bundle --format=esm --minify --outfile=dist/sw.js
cp node_modules/spessasynth_lib/dist/spessasynth_processor.min.js dist/lib/
rm -rf dist/lib/libarchive && mkdir -p dist/lib/libarchive
cp node_modules/libarchive.js/dist/worker-bundle.js node_modules/libarchive.js/dist/libarchive.wasm dist/lib/libarchive/
rm -rf dist/lib/music.js dist/index.html dist/style.css dist/lang.js dist/desktop.js dist/favicon.svg dist/zlaty-kalich
echo "Player bundle in web/dist (served at /zlaty-kalich/ on the site)"
