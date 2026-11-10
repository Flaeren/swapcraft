#!/bin/bash
# Linux counterpart of fetch-engines.ps1: downloads the pinned Linux build of each ArtCraft engine named in
# engines.json (the `linux` entry), refuses it unless its SHA-256 is the pinned one, and lays it out in
# src/engines/<name>/ the way the apps expect: the command-line program at the top, its licences and notices next
# to it, and its data (the OCR models of PdfCraft) in the folder it reads them from. The engine's own window
# program, icons, man pages and shell completions are left out.
#
#   ./fetch-engines.sh                  the seven downloaded engines
#   ./fetch-engines.sh --build-cadcraft also CADCraft, built from source at the pinned commit with our patch
#
# The downloads are kept in src/engines/.cache, so a second run downloads nothing. Needs curl, jq, sha256sum, git.
set -euo pipefail
src="$(cd "$(dirname "$0")" && pwd)"
root="$src/engines"
cache="$root/.cache"
cfg="$src/engines.json"
mkdir -p "$cache"

build_cadcraft=0
[ "${1:-}" = "--build-cadcraft" ] && build_cadcraft=1

n=$(jq '.engines | length' "$cfg")
for i in $(seq 0 $((n - 1))); do
  name=$(jq -r ".engines[$i].name" "$cfg")
  repo=$(jq -r ".engines[$i].repo" "$cfg")
  tag=$(jq -r ".engines[$i].tag" "$cfg")
  asset=$(jq -r ".engines[$i].linux.asset // empty" "$cfg")
  sha=$(jq -r ".engines[$i].linux.sha256 // empty" "$cfg")
  if [ -z "$asset" ]; then echo "$name has no Linux asset in engines.json" >&2; exit 1; fi
  file="$cache/$asset"
  if [ ! -f "$file" ]; then
    echo "$name $tag: downloading $asset"
    curl -fsSL -o "$file.part" "https://github.com/$repo/releases/download/$tag/$asset"
    mv "$file.part" "$file"
  fi
  got=$(sha256sum "$file" | cut -d' ' -f1)
  if [ "$got" != "$sha" ]; then rm -f "$file"; echo "$asset: SHA-256 $got, expected $sha; refused" >&2; exit 1; fi

  dest="$root/$name"
  work="$root/.unpack-$name"
  rm -rf "$dest" "$work"; mkdir -p "$dest" "$work"
  tar xzf "$file" -C "$work"
  top=$(find "$work" -mindepth 1 -maxdepth 1 -type d | head -1)
  [ -x "$top/bin/$name-cli" ] || { echo "$asset has no bin/$name-cli" >&2; exit 1; }
  cp -p "$top/bin/$name-cli" "$dest/"
  # licences, notices, attributions (README too: it is the engine's own documentation)
  [ -d "$top/share/doc/$name" ] && cp -p "$top/share/doc/$name"/* "$dest/"
  # the engine's data folder (PdfCraft: models/), which the program finds beside itself
  [ -d "$top/share/$name" ] && cp -pr "$top/share/$name"/. "$dest/"
  rm -rf "$work"
  own="$src/licenses/$name"
  [ -d "$own" ] && cp -p "$own"/* "$dest/"
  echo "$name $tag ready: $("$dest/$name-cli" --version 2>/dev/null | head -1)"
done

if [ "$build_cadcraft" = 1 ]; then
  repo=$(jq -r '.cadcraft.repo' "$cfg"); tag=$(jq -r '.cadcraft.tag' "$cfg"); commit=$(jq -r '.cadcraft.commit' "$cfg")
  patch=$(jq -r '.cadcraft.patch' "$cfg"); package=$(jq -r '.cadcraft.package' "$cfg")
  echo "cadcraft $tag from source, with $patch"
  work="$root/.cadcraft-src"
  rm -rf "$work"
  git clone --quiet --depth 1 --branch "$tag" "https://github.com/$repo.git" "$work"
  head=$(git -C "$work" rev-parse HEAD)
  [ "$head" = "$commit" ] || { echo "tag $tag points at $head, the pinned commit is $commit; refusing it" >&2; exit 1; }
  git -C "$work" apply "$src/$patch" || { echo "the patch does not apply to $tag" >&2; exit 1; }
  (cd "$work" && cargo build --release -p "$package")
  dest="$root/cadcraft"
  rm -rf "$dest"; mkdir -p "$dest"
  cp -p "$work/target/release/cadcraft-cli" "$dest/"
  for f in LICENSE-MIT LICENSE-APACHE NOTICE ATTRIBUTION.md; do cp -p "$work/$f" "$dest/"; done
  rm -rf "$work"
  echo "cadcraft ready: $("$dest/cadcraft-cli" --version 2>/dev/null | head -1)"
fi
echo "engines ready in $root"
