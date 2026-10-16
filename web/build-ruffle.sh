#!/usr/bin/env bash
# Builds Ruffle's self-hosted web player with the vendored swf change, defaulting
# SWF5-and-older text to Windows-1250 (Czech). Output: web/dist/ruffle/.
# Needs: node/npm, rustup target wasm32-unknown-unknown, wasm-bindgen-cli
# matching Ruffle's lockfile, Java (Ruffle builds its AVM2 globals), and
# optionally binaryen's wasm-opt.
set -euo pipefail
cd "$(dirname "$0")/.."

rev=387ea68a46891b28e1579bc53725263396b4be38
src=target/ruffle-web
if [ ! -d "$src/.git" ]; then
  git clone --filter=blob:none https://github.com/ruffle-rs/ruffle.git "$src"
fi
git -C "$src" fetch --quiet origin "$rev" || true
git -C "$src" checkout --quiet --force "$rev"

# Same change as vendor/swf, with cp1250 as the default (the web player has no
# host code to set it).
cp vendor/swf/src/string.rs "$src/swf/src/string.rs"
sed -i.bak 's/LEGACY_ENCODING.get().copied().unwrap_or(WINDOWS_1252)/LEGACY_ENCODING.get().copied().unwrap_or(encoding_rs::WINDOWS_1250)/' \
  "$src/swf/src/string.rs"
rm "$src/swf/src/string.rs.bak"
grep -q 'unwrap_or(encoding_rs::WINDOWS_1250)' "$src/swf/src/string.rs"

(
  cd "$src/web"
  npm ci --no-audit --no-fund
  npm run build --workspace=ruffle-core
  npm run build --workspace=ruffle-selfhosted
)
rm -rf web/dist/ruffle
mkdir -p web/dist
cp -R "$src/web/packages/selfhosted/dist" web/dist/ruffle
echo "Ruffle web player in web/dist/ruffle"
