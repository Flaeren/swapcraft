#!/bin/bash
# Linux and macOS counterpart of fetch-engines.ps1: downloads the pinned Linux (or macOS, universal) build of each
# ArtCraft engine named in engines.json (the `linux` or `macos` entry), refuses it unless its SHA-256 is the pinned one, and lays it out in
# src/engines/<name>/ the way the apps expect: the command-line program at the top, its licences and notices next
# to it, and its data (the OCR models of PdfCraft) in the folder it reads them from. The engine's own window
# program, icons, man pages and shell completions are left out.
#
#   ./fetch-engines.sh                  the seven downloaded engines
#   ./fetch-engines.sh --build-cadcraft also CADCraft, built from source at the pinned commit with our patch
#
# The downloads are kept in src/engines/.cache, so a second run downloads nothing. Needs curl, jq, sha256sum, git
# (on macOS: GNU coreutils first on the PATH, and unzip and lipo, which the system has).
set -euo pipefail
case "$(uname -s)" in Darwin) os=macos ;; *) os=linux ;; esac
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
  asset=$(jq -r ".engines[$i].$os.asset // empty" "$cfg")
  sha=$(jq -r ".engines[$i].$os.sha256 // empty" "$cfg")
  if [ -z "$asset" ]; then echo "$name has no $os asset in engines.json" >&2; exit 1; fi
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
  if [ "$os" = macos ]; then
    # the macOS command-line ZIP is flat: the program, its notices and any data side by side in one folder
    unzip -q "$file" -d "$work"
    top=$(find "$work" -mindepth 1 -maxdepth 1 -type d | head -1)
    [ -f "$top/$name-cli" ] || { echo "$asset has no $name-cli" >&2; exit 1; }
    cp -pR "$top"/. "$dest/"
    chmod 755 "$dest/$name-cli"
    # data the macOS ZIP leaves out (PdfCraft: models/ for OCR) is the same on every system: from the Linux package
    if [ "$(jq -r ".engines[$i].macos.data_from_linux // false" "$cfg")" = true ]; then
      lasset=$(jq -r ".engines[$i].linux.asset" "$cfg"); lsha=$(jq -r ".engines[$i].linux.sha256" "$cfg")
      lfile="$cache/$lasset"
      [ -f "$lfile" ] || { curl -fsSL -o "$lfile.part" "https://github.com/$repo/releases/download/$tag/$lasset"; mv "$lfile.part" "$lfile"; }
      got=$(sha256sum "$lfile" | cut -d' ' -f1)
      if [ "$got" != "$lsha" ]; then rm -f "$lfile"; echo "$lasset: SHA-256 $got, expected $lsha; refused" >&2; exit 1; fi
      mkdir -p "$work/linux"; tar xzf "$lfile" -C "$work/linux"
      ltop=$(find "$work/linux" -mindepth 1 -maxdepth 1 -type d | head -1)
      [ -d "$ltop/share/$name" ] || { echo "$lasset has no share/$name" >&2; exit 1; }
      cp -pR "$ltop/share/$name"/. "$dest/"
    fi
  else
    tar xzf "$file" -C "$work"
    top=$(find "$work" -mindepth 1 -maxdepth 1 -type d | head -1)
    [ -x "$top/bin/$name-cli" ] || { echo "$asset has no bin/$name-cli" >&2; exit 1; }
    cp -p "$top/bin/$name-cli" "$dest/"
    # licences, notices, attributions (README too: it is the engine's own documentation)
    if [ -d "$top/share/doc/$name" ]; then cp -p "$top/share/doc/$name"/* "$dest/"; fi
    # the engine's data folder (PdfCraft: models/), which the program finds beside itself
    if [ -d "$top/share/$name" ]; then cp -pr "$top/share/$name"/. "$dest/"; fi
  fi
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
  dest="$root/cadcraft"
  rm -rf "$dest"; mkdir -p "$dest"
  if [ "$os" = macos ]; then
    # one program for both kinds of Mac (Apple silicon and Intel), like the downloaded engines
    (cd "$work" && cargo build --release -p "$package" --target aarch64-apple-darwin && cargo build --release -p "$package" --target x86_64-apple-darwin)
    lipo -create -output "$dest/cadcraft-cli" "$work/target/aarch64-apple-darwin/release/cadcraft-cli" "$work/target/x86_64-apple-darwin/release/cadcraft-cli"
  else
    (cd "$work" && cargo build --release -p "$package")
    cp -p "$work/target/release/cadcraft-cli" "$dest/"
  fi
  for f in LICENSE-MIT LICENSE-APACHE NOTICE ATTRIBUTION.md; do cp -p "$work/$f" "$dest/"; done
  rm -rf "$work"
  echo "cadcraft ready: $("$dest/cadcraft-cli" --version 2>/dev/null | head -1)"
fi
echo "engines ready in $root"
