#!/bin/bash
# Linux counterpart of build-apps.ps1: builds the hub and the apps in release and lays out ../portable-linux, the
# folder the Linux release is packed from:
#   ../portable-linux/swapcraft             the hub (and swapcraft-cli, swapcraft-tray, swapcraft.png for the menu launcher)
#   ../portable-linux/apps/<id>/<id>         each app, with its engine next to it (apps/<id>/<engine>/)
# On macOS the same layout goes to ../portable-macos, every program built for Apple silicon and for Intel and joined
# into one (universal) file with lipo; there it needs bash 4 or later and GNU coreutils first on the PATH.
# Run fetch-engines.sh --build-cadcraft first. Usage: ./build-apps.sh
set -euo pipefail
src="$(cd "$(dirname "$0")" && pwd)"
cd "$src"
if [ "$(uname -s)" = Darwin ]; then
  out="$(dirname "$src")/portable-macos"
  cargo build --release --workspace --target aarch64-apple-darwin
  cargo build --release --workspace --target x86_64-apple-darwin
  bin=target/universal
  mkdir -p "$bin"
  for f in target/aarch64-apple-darwin/release/*; do
    name=$(basename "$f")
    if [ -f "$f" ] && [ -x "$f" ] && [ -f "target/x86_64-apple-darwin/release/$name" ]; then
      lipo -create -output "$bin/$name" "$f" "target/x86_64-apple-darwin/release/$name"
    fi
  done
else
  out="$(dirname "$src")/portable-linux"
  cargo build --release --workspace
  bin=target/release
fi
mkdir -p "$out"

# a running program can be replaced on Linux (the old file lives on until it exits), so a plain copy is enough
install -m 755 "$bin/voidflare-hub" "$out/swapcraft"
install -m 755 "$bin/swapcraft-cli" "$out/swapcraft-cli"
# the drop bar and tray icon (resident helper): Linux (X11) only for now; the Mac build of it is a stub that only says so
if [ "$(uname -s)" != Darwin ] && [ -x "$bin/swapcraft-tray" ]; then install -m 755 "$bin/swapcraft-tray" "$out/swapcraft-tray"; fi
install -m 644 hub/assets/swapcraft-256.png "$out/swapcraft.png"
echo "swapcraft, swapcraft-cli, swapcraft-tray (Linux), swapcraft.png"

declare -A engines=(
  [word-to-pdf]=wordcraft [excel-to-pdf]=gridcraft [slides-to-pdf]=deckcraft [pdf-tools]=pdfcraft
  [dwg-to-pdf]=cadcraft [image-converter]=photocraft [audio-converter]=soundcraft [video-converter]=filmcraft
  [ebook-converter]=wordcraft [mail-converter]="wordcraft mailcraft"
)
for dir in crates/app-*/; do
  id=$(basename "$dir"); id=${id#app-}
  dest="$out/apps/$id"
  mkdir -p "$dest"
  install -m 755 "$bin/$id" "$dest/$id"
  echo "apps/$id/$id"
  # Mail Converter carries two: wordcraft (its pack engine) and mailcraft (inside its app ZIP, for .pst files)
  for engine in ${engines[$id]:-}; do
    # Each app carries its own engine, so nobody needs ArtCraft installed: the command-line program, its data and
    # notices, and the MIT/Apache licence texts. fetch-engines.sh already left out the engine's own window.
    installed="$src/engines/$engine"
    [ -x "$installed/$engine-cli" ] || { echo "$engine is not in $installed: run fetch-engines.sh first" >&2; exit 1; }
    rm -rf "$dest/$engine"
    cp -pr "$installed" "$dest/$engine"
    if [ -d "licenses/$engine" ]; then cp -p "licenses/$engine"/* "$dest/$engine/"; fi
    echo "  + $engine/"
  done
done
