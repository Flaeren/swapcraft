# Packs ..\portable into the files a GitHub release carries, in ..\release\ :
#   voidflare-<ver>-windows.zip              the FULL bundle (hub + every app + its engine). Keeps the name 0.1.0 hubs look
#                                            for, and is the download for a PC without a network
#   voidflare-hub-<ver>-windows.zip          the hub alone: pick what you need from its cards
#   voidflare-app-<id>-<ver>-windows-x64.zip     one app (its exe), no engine
#   voidflare-engine-<name>-<engine ver>-windows-x64.zip   one trimmed engine (cli, models, notices, licences)
#   (every name starts with "voidflare-" and the full bundle sorts first: a 0.1.0 hub takes the first ZIP of a release)
#   manifest.json                            what the hub reads: packs, their assets, which PC engines are tested-compatible
#   SHA256SUMS.txt                           "<hash>  <file name>" for every file above, the format the hub checks against
# It does NOT publish anything. To publish (after trying it): see the "Release" part of CLAUDE.md.
# Usage: powershell -File release.ps1   (run build-apps.ps1 first)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$src = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Split-Path -Parent $src
$portable = Join-Path $root "portable"
$out = Join-Path $root "release"
if (-not (Test-Path (Join-Path $portable "voidflare.exe"))) { throw "portable\voidflare.exe is missing: run build-apps.ps1 first" }
$version = (Select-String -Path (Join-Path $src "Cargo.toml") -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
# Version guard: the hub and every app must carry the release version in their own file properties, and the
# CHANGELOG must have a section for it. (In 0.2.0 the hub's Cargo.toml was left at 0.1.0: it shipped saying "v0.1.0" and
# would have offered its own version as an update forever.) A release is refused when any of them disagrees.
$mismatch = @()
Get-ChildItem $portable -Recurse -Filter *.exe | Where-Object { $_.Name -notlike "*.old.exe" -and $_.DirectoryName -notmatch '\\(wordcraft|gridcraft|deckcraft|pdfcraft|cadcraft|photocraft|soundcraft|filmcraft)(\\|$)' } | ForEach-Object {
    if ($_.VersionInfo.ProductVersion -ne $version) { $mismatch += "$($_.FullName.Substring($portable.Length + 1)) says '$($_.VersionInfo.ProductVersion)'" }
}
if ($mismatch) { throw "Version $version (src\Cargo.toml) but: " + ($mismatch -join "; ") + ". Run build-apps.ps1 again; if it persists a Cargo.toml has its own version instead of version.workspace = true." }
if (-not (Select-String -Path (Join-Path $root "CHANGELOG.md") -SimpleMatch "## [$version]" -Quiet)) { throw "CHANGELOG.md has no '## [$version]' section: write the release notes (and move [Unreleased] under it) before releasing." }
Write-Host "Version ${version}: hub and apps agree, CHANGELOG has its section."
$engines = Get-Content -Raw (Join-Path $src "engines.json") | ConvertFrom-Json
$compat = Get-Content -Raw (Join-Path $src "engine-compat.json") | ConvertFrom-Json
# Display names live in hub\src\catalog.rs; keep these in step with it.
$names = @{ "word-to-pdf" = "Word to PDF"; "excel-to-pdf" = "Excel to PDF"; "slides-to-pdf" = "Slides to PDF"; "pdf-tools" = "PDF Tools"; "dwg-to-pdf" = "DWG to PDF"; "image-converter" = "Image Converter"; "audio-converter" = "Audio Converter"; "video-converter" = "Video Converter"; "ebook-converter" = "E-book Converter" }
if (Test-Path $out) { Remove-Item -LiteralPath $out -Recurse -Force }
New-Item -ItemType Directory -Force $out | Out-Null

# Zips a folder's files (not the folder itself) with forward-slash entry names and a stable order. $skip decides what is left out.
function New-Zip([string]$Zip, [string]$Dir, [scriptblock]$Skip) {
    $base = (Resolve-Path -LiteralPath $Dir).Path.TrimEnd('\')
    $archive = [System.IO.Compression.ZipFile]::Open($Zip, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        Get-ChildItem -LiteralPath $base -Recurse -File -Force | Sort-Object FullName | ForEach-Object {
            $rel = $_.FullName.Substring($base.Length + 1).Replace('\', '/')
            if (& $Skip $rel) { return }
            [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $_.FullName, $rel, [System.IO.Compression.CompressionLevel]::Optimal)
        }
    } finally { $archive.Dispose() }
    if ((Get-Item $Zip).Length -lt 100) { throw "$Zip came out empty" }
}
function Size-Mb([string]$p) { "{0:N1} MB" -f ((Get-Item $p).Length / 1MB) }

# Leftovers of a hub replaced while it ran (*.old.exe) and the install record the hub writes into a pack it installed.
$leftovers = { param($rel) $rel -like "*.old.exe" -or $rel -eq "pack.json" -or $rel -like "*/pack.json" }

# 1. full bundle, 2. hub alone
$full = Join-Path $out "voidflare-$version-windows.zip"
New-Zip $full $portable $leftovers
Write-Host ("{0,-52} {1}" -f (Split-Path -Leaf $full), (Size-Mb $full))
$hubOnly = Join-Path $out "voidflare-hub-$version-windows.zip"
New-Zip $hubOnly $portable { param($rel) $rel -notin @("voidflare.exe", "voidflare-cli.exe") }
Write-Host ("{0,-52} {1}" -f (Split-Path -Leaf $hubOnly), (Size-Mb $hubOnly))

# 3. one app zip (+ 4. one engine zip) per app in portable\apps\
$packs = @()
foreach ($appDir in (Get-ChildItem (Join-Path $portable "apps") -Directory | Sort-Object Name)) {
    $id = $appDir.Name
    if (-not (Test-Path (Join-Path $appDir.FullName "$id.exe"))) { throw "apps\$id\$id.exe is missing" }
    $def = $engines.engines | Where-Object { $_.app -eq $id -or $_.also -contains $id } | Select-Object -First 1
    if (-not $def -and $engines.cadcraft.app -eq $id) { $def = $engines.cadcraft }
    $engineName = if ($def) { $def.name } else { $null }

    $appZip = Join-Path $out "voidflare-app-$id-$version-windows-x64.zip"
    $engineLeft = if ($engineName) { $engineName + "/" } else { "`0" }
    New-Zip $appZip $appDir.FullName { param($rel) (& $leftovers $rel) -or $rel.StartsWith($engineLeft) }
    Write-Host ("{0,-52} {1}" -f (Split-Path -Leaf $appZip), (Size-Mb $appZip))
    $pack = [ordered]@{ id = $id; name = $names[$id]; version = $version; app_asset = (Split-Path -Leaf $appZip) }
    if (-not $pack.name) { throw "No display name for ${id}: add it to `$names" }

    if ($engineName) {
        $engineDir = Join-Path $appDir.FullName $engineName
        $cli = "$engineName-cli.exe"
        if (-not (Test-Path (Join-Path $engineDir $cli))) { throw "apps\$id\$engineName\$cli is missing" }
        $engineVersion = $def.tag.TrimStart('v')
        $engineZip = Join-Path $out "voidflare-engine-$engineName-$engineVersion-windows-x64.zip"
        # an engine shared by two apps (WordCraft: Word to PDF and E-book Converter) is zipped once
        if (-not (Test-Path $engineZip)) { New-Zip $engineZip $engineDir $leftovers }
        Write-Host ("{0,-52} {1}" -f (Split-Path -Leaf $engineZip), (Size-Mb $engineZip))
        $tested = @(); if ($compat.PSObject.Properties.Name -contains $engineName) { $tested = @($compat.$engineName) }
        $pack.engine = [ordered]@{ name = $engineName; folder = $engineName; cli = $cli; version = $engineVersion; asset = (Split-Path -Leaf $engineZip); compatible = $tested }
    }
    $packs += , $pack
}

# 5. the manifest and 6. the checksums (the manifest is listed in them, so the hub can trust it as much as the zips)
$manifest = Join-Path $out "manifest.json"
$noBom = New-Object System.Text.UTF8Encoding($false)
$json = ([ordered]@{ schema = 1; version = $version; packs = $packs } | ConvertTo-Json -Depth 8)
[System.IO.File]::WriteAllText($manifest, $json + "`n", $noBom)
$lines = foreach ($f in (Get-ChildItem $out -File | Sort-Object Name)) {
    "{0}  {1}" -f (Get-FileHash -Algorithm SHA256 -LiteralPath $f.FullName).Hash.ToLower(), $f.Name
}
# no BOM, LF line endings: sha256sum -c and the hub both read this
[System.IO.File]::WriteAllText((Join-Path $out "SHA256SUMS.txt"), (($lines -join "`n") + "`n"), $noBom)
Write-Host ("manifest.json + SHA256SUMS.txt ({0} files listed)" -f @($lines).Count)
