#!/bin/bash
# Linux counterpart of build-apps.ps1: builds the hub and the apps in release and lays out ../portable-linux, the
# folder the Linux release is packed from:
#   ../portable-linux/swapcraft             the hub (and swapcraft-cli, swapcraft.png for the menu launcher)
#   ../portable-linux/apps/<id>/<id>         each app, with its engine next to it (apps/<id>/<engine>/)
# Run fetch-engines.sh --build-cadcraft first. Usage: ./build-apps.sh
set -euo pipefail
src="$(cd "$(dirname "$0")" && pwd)"
out="$(dirname "$src")/portable-linux"
mkdir -p "$out"
cd "$src"
cargo build --release --workspace

# a running program can be replaced on Linux (the old file lives on until it exits), so a plain copy is enough
install -m 755 target/release/voidflare-hub "$out/swapcraft"
install -m 755 target/release/swapcraft-cli "$out/swapcraft-cli"
install -m 644 hub/assets/swapcraft-256.png "$out/swapcraft.png"
echo "swapcraft, swapcraft-cli, swapcraft.png"

declare -A engines=(
  [word-to-pdf]=wordcraft [excel-to-pdf]=gridcraft [slides-to-pdf]=deckcraft [pdf-tools]=pdfcraft
  [dwg-to-pdf]=cadcraft [image-converter]=photocraft [audio-converter]=soundcraft [video-converter]=filmcraft
  [ebook-converter]=wordcraft
)
for dir in crates/app-*/; do
  id=$(basename "$dir"); id=${id#app-}
  dest="$out/apps/$id"
  mkdir -p "$dest"
  install -m 755 "target/release/$id" "$dest/$id"
  echo "apps/$id/$id"
  engine=${engines[$id]:-}
  [ -z "$engine" ] && continue
  # Each app carries its own engine, so nobody needs ArtCraft installed: the command-line program, its data and
  # notices, and the MIT/Apache licence texts. fetch-engines.sh already left out the engine's own window.
  installed="$src/engines/$engine"
  [ -x "$installed/$engine-cli" ] || { echo "$engine is not in $installed: run fetch-engines.sh first" >&2; exit 1; }
  rm -rf "$dest/$engine"
  cp -pr "$installed" "$dest/$engine"
  [ -d "licenses/$engine" ] && cp -p "licenses/$engine"/* "$dest/$engine/"
  echo "  + $engine/"
done
