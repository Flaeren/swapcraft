#!/bin/bash
# Linux counterpart of release.ps1: packs ../portable-linux into the Linux files of a GitHub release, in ../release/ :
#   swapcraft-<ver>-linux-x86_64.zip                the FULL bundle (hub + every app + its engine), for a PC without a network
#   swapcraft-hub-<ver>-linux-x86_64.zip            the hub alone (swapcraft, swapcraft-cli, swapcraft.png)
#   swapcraft-app-<id>-<ver>-linux-x86_64.zip       one app (its program), no engine
#   swapcraft-engine-<name>-<engine ver>-linux-x86_64.zip   one trimmed engine (cli, data, notices, licences)
#   manifest-linux-x86_64.json                      what the Linux hub reads (the Windows hub reads manifest.json)
#   SHA256SUMS.txt                                  "<hash>  <file name>" for every file above
# ZIP, not tar.gz, for all of them: the hub unpacks the same format on both systems, and the permissions are kept
# (the programs stay runnable). In CI the Windows and Linux files are put together and SHA256SUMS.txt is written again
# over all of them. Nothing is published here. Usage: ./release.sh   (run build-apps.sh first)
set -euo pipefail
src="$(cd "$(dirname "$0")" && pwd)"
root="$(dirname "$src")"
portable="$root/portable-linux"
out="$root/release"
tag=linux-x86_64
[ -x "$portable/swapcraft" ] || { echo "portable-linux/swapcraft is missing: run build-apps.sh first" >&2; exit 1; }
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$src/Cargo.toml" | head -1)

# Version guard, as on Windows: every crate takes the workspace version, the command line reports it, the CHANGELOG
# has a section for it.
stray=$(grep -L 'version.workspace = true' "$src"/hub/Cargo.toml "$src"/crates/*/Cargo.toml || true)
[ -z "$stray" ] || { echo "these crates have their own version instead of version.workspace = true: $stray" >&2; exit 1; }
cli_version=$("$portable/swapcraft-cli" --version | grep -o '[0-9][0-9.]*' | head -1)
[ "$cli_version" = "$version" ] || { echo "Version $version (src/Cargo.toml) but swapcraft-cli says $cli_version: run build-apps.sh again" >&2; exit 1; }
grep -qF "## [$version]" "$root/CHANGELOG.md" || { echo "CHANGELOG.md has no '## [$version]' section" >&2; exit 1; }
echo "Version $version: crates and command line agree, CHANGELOG has its section."

rm -rf "$out"; mkdir -p "$out"

# zip_dir <zip> <dir> [grep -v pattern]: the folder's files (not the folder itself), sorted, permissions kept
zip_dir() {
  local zip=$1 dir=$2 skip=${3:-'^$'}
  (cd "$dir" && find . -type f | sed 's|^\./||' | grep -Ev "$skip" | LC_ALL=C sort | zip -q -X -9 "$zip" -@)
  [ "$(stat -c %s "$zip")" -gt 100 ] || { echo "$zip came out empty" >&2; exit 1; }
  printf '%-56s %s\n' "$(basename "$zip")" "$(du -h "$zip" | cut -f1)"
}
leftovers='(^|/)pack\.json$|\.old$'

zip_dir "$out/swapcraft-$version-$tag.zip" "$portable" "$leftovers"
(cd "$portable" && zip -q -X -9 "$out/swapcraft-hub-$version-$tag.zip" swapcraft swapcraft-cli swapcraft.png)
printf '%-56s %s\n' "swapcraft-hub-$version-$tag.zip" "$(du -h "$out/swapcraft-hub-$version-$tag.zip" | cut -f1)"

declare -A names=(
  [word-to-pdf]="Word to PDF" [excel-to-pdf]="Excel to PDF" [slides-to-pdf]="Slides to PDF" [pdf-tools]="PDF Tools"
  [dwg-to-pdf]="DWG to PDF" [image-converter]="Image Converter" [audio-converter]="Audio Converter"
  [video-converter]="Video Converter" [ebook-converter]="E-book Converter"
)
packs=()
for dir in $(find "$portable/apps" -mindepth 1 -maxdepth 1 -type d | LC_ALL=C sort); do
  id=$(basename "$dir")
  [ -x "$dir/$id" ] || { echo "apps/$id/$id is missing" >&2; exit 1; }
  name=${names[$id]:-}; [ -n "$name" ] || { echo "No display name for $id" >&2; exit 1; }
  engine=$(jq -r --arg id "$id" '(.engines[] | select(.app == $id or ((.also // []) | index($id)))) .name' "$src/engines.json" | head -1)
  [ -z "$engine" ] && [ "$(jq -r .cadcraft.app "$src/engines.json")" = "$id" ] && engine=cadcraft
  app_zip="swapcraft-app-$id-$version-$tag.zip"
  if [ -n "$engine" ]; then zip_dir "$out/$app_zip" "$dir" "$leftovers|^$engine/"; else zip_dir "$out/$app_zip" "$dir" "$leftovers"; fi
  pack=$(jq -n --arg id "$id" --arg name "$name" --arg v "$version" --arg a "$app_zip" '{id: $id, name: $name, version: $v, app_asset: $a}')
  if [ -n "$engine" ]; then
    [ -x "$dir/$engine/$engine-cli" ] || { echo "apps/$id/$engine/$engine-cli is missing" >&2; exit 1; }
    if [ "$engine" = cadcraft ]; then ev=$(jq -r .cadcraft.tag "$src/engines.json"); else ev=$(jq -r --arg n "$engine" '.engines[] | select(.name == $n) | .tag' "$src/engines.json"); fi
    ev=${ev#v}
    engine_zip="swapcraft-engine-$engine-$ev-$tag.zip"
    # an engine shared by two apps (WordCraft: Word to PDF and E-book Converter) is zipped once
    [ -f "$out/$engine_zip" ] || zip_dir "$out/$engine_zip" "$dir/$engine" "$leftovers"
    tested=$(jq -c --arg n "$engine" '.[$n] // []' "$src/engine-compat.json")
    pack=$(echo "$pack" | jq --arg n "$engine" --arg ev "$ev" --arg a "$engine_zip" --argjson t "$tested" \
      '. + {engine: {name: $n, folder: $n, cli: ($n + "-cli"), version: $ev, asset: $a, compatible: $t}}')
  fi
  packs+=("$pack")
done
printf '%s\n' "${packs[@]}" | jq -s --arg v "$version" '{schema: 1, version: $v, packs: .}' > "$out/manifest-$tag.json"
(cd "$out" && LC_ALL=C ls -1 | grep -v '^SHA256SUMS.txt$' | xargs sha256sum > SHA256SUMS.txt)
echo "manifest-$tag.json + SHA256SUMS.txt ($(wc -l < "$out/SHA256SUMS.txt") files listed)"
