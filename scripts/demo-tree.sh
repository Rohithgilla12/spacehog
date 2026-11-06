#!/usr/bin/env bash
# Builds a synthetic home directory for screenshots and GIFs, so nothing
# private shows and nothing real is ever cleaned.
#
#   scripts/demo-tree.sh DIR          # fresh tree at DIR (about 3 GB)
#   scripts/demo-tree.sh DIR grow     # add growth for the Changes view
#
# DEMO_MB scales every file (default 1 = sizes below are in MB). Point HOME
# at DIR when running spacehog so paths read as ~/code/... and the default
# rules recognise ~/Library and ~/.npm.
set -euo pipefail

dir=$1
mode=${2:-fresh}
scale=${DEMO_MB:-1}

blob() {
  mkdir -p "$(dirname "$dir/$1")"
  head -c $(($2 * scale * 1000 * 1000)) /dev/zero > "$dir/$1"
}

text() {
  mkdir -p "$(dirname "$dir/$1")"
  printf '%s\n' "$2" > "$dir/$1"
}

# Ages a path and everything under it by N days, so staleness is real.
age() {
  local stamp
  stamp=$(date -v-"$2"d +%Y%m%d%H%M 2>/dev/null || date -d "-$2 days" +%Y%m%d%H%M)
  find "$dir/$1" -exec touch -h -t "$stamp" {} +
}

if [[ "$mode" == grow ]]; then
  blob code/webshop/node_modules/@next/swc-darwin-arm64/next-swc.node 260
  blob Downloads/ubuntu-24.04-desktop.iso 420
  blob .cache/uv/wheels/torch-2.5.whl 180
  rm -rf "$dir/code/mobile/ios/Pods"
  exit 0
fi

rm -rf "$dir"
mkdir -p "$dir"

text code/webshop/package.json '{"name":"webshop","scripts":{"build":"next build"}}'
text code/webshop/pnpm-lock.yaml 'lockfileVersion: 9.0'
text code/webshop/.git/HEAD 'ref: refs/heads/main'
text code/webshop/src/app/page.tsx 'export default function Page() { return null }'
blob code/webshop/node_modules/next/dist/compiled.js 140
blob code/webshop/node_modules/@img/sharp-darwin-arm64/sharp.node 60
blob code/webshop/node_modules/typescript/lib/typescript.js 45
blob code/webshop/node_modules/react-dom/cjs/react-dom.js 20
blob code/webshop/node_modules/esbuild/bin/esbuild 30
blob code/webshop/.next/cache/webpack/client.pack 120

text code/api-rs/Cargo.toml '[package]
name = "api"'
text code/api-rs/Cargo.lock 'version = 3'
text code/api-rs/.git/HEAD 'ref: refs/heads/main'
text code/api-rs/src/main.rs 'fn main() {}'
blob code/api-rs/target/debug/deps/libtokio.rlib 160
blob code/api-rs/target/debug/incremental/api.bin 140
blob code/api-rs/target/release/api 90

text code/ml-lab/pyproject.toml '[project]
name = "ml-lab"'
text code/ml-lab/uv.lock 'version = 1'
text code/ml-lab/.git/HEAD 'ref: refs/heads/main'
text code/ml-lab/.venv/pyvenv.cfg 'home = /usr/bin'
blob code/ml-lab/.venv/lib/python3.12/site-packages/torch/lib/libtorch.dylib 260
blob code/ml-lab/.venv/lib/python3.12/site-packages/numpy/core/multiarray.so 40
blob code/ml-lab/src/__pycache__/train.cpython-312.pyc 8

text code/old-dashboard/package.json '{"name":"old-dashboard"}'
text code/old-dashboard/package-lock.json '{}'
text code/old-dashboard/.git/HEAD 'ref: refs/heads/main'
blob code/old-dashboard/node_modules/webpack/lib/bundle.js 120
blob code/old-dashboard/node_modules/@babel/core/lib/index.js 70
blob code/old-dashboard/node_modules/core-js/modules/all.js 40
age code/old-dashboard 240

text code/mobile/ios/Podfile "platform :ios, '17.0'"
text code/mobile/.git/HEAD 'ref: refs/heads/main'
blob code/mobile/ios/Pods/Firebase/Firebase.framework 90
blob code/mobile/ios/Pods/GoogleMaps/GoogleMaps.framework 50
age code/mobile 130

blob Downloads/Xcode_16.dmg 380
blob Downloads/conference-slides.zip 60
blob Downloads/old-installer.pkg 90
age Downloads/Xcode_16.dmg 150
age Downloads/old-installer.pkg 300

blob Library/Caches/Homebrew/downloads/llvm.bottle.tar.gz 150
blob Library/Caches/com.spotify.client/Data/cache.bin 70
blob Library/Developer/Xcode/DerivedData/Mobile-abc/Build/Intermediates.noindex/x.o 210
blob Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw 480
blob .npm/_cacache/content-v2/sha512/packs.bin 140
blob .cache/uv/archive-v0/wheels.bin 100
blob .cache/huggingface/hub/models--demo--small/blobs/model.safetensors 230

blob Movies/holiday-2025.mp4 180
blob Pictures/Photos/raw-shoot.cr3 90
text Documents/notes.md '# notes'
blob Documents/thesis.pdf 25
