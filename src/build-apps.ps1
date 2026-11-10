# Builds the hub and the Rust apps in release and puts the products in ..\portable, the only folder
# that needs zipping to try SwapCraft on another PC:
#   ..\portable\SwapCraft.exe          the hub (and swapcraft-cli.exe, voidflare_dragdrop.dll)
#   ..\portable\apps\<id>\<id>.exe     each app, with its engine next to it
# Usage: powershell -File build-apps.ps1
$ErrorActionPreference = "Stop"
$src = Split-Path -Parent $MyInvocation.MyCommand.Path
$out = Join-Path (Split-Path -Parent $src) "portable"
New-Item -ItemType Directory -Force $out | Out-Null
$cargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
Push-Location $src
try {
    & $cargo build --release --workspace
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    # a running exe cannot be overwritten but can be renamed: put $file in place of $dest, parking the old one
    function Put([string]$file, [string]$dest) {
        $old = [System.IO.Path]::ChangeExtension($dest, ".old" + [System.IO.Path]::GetExtension($dest))
        if (Test-Path $old) { Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue }
        if (Test-Path $dest) { try { Remove-Item -LiteralPath $dest -Force -ErrorAction Stop } catch { Move-Item -LiteralPath $dest -Destination $old -Force } }
        Copy-Item $file $dest -Force
        Write-Host (Split-Path -Leaf $dest)
    }
    # since 0.5 the hub is SwapCraft.exe and the command line swapcraft-cli.exe; the 0.4 names go
    foreach ($legacy in @("voidflare.exe", "voidflare-cli.exe")) {
        $p = Join-Path $out $legacy
        if (Test-Path $p) { try { Remove-Item -LiteralPath $p -Force -ErrorAction Stop } catch { Move-Item -LiteralPath $p -Destination ([System.IO.Path]::ChangeExtension($p, ".old.exe")) -Force } }
    }
    Put (Join-Path $src "target\release\voidflare-hub.exe") (Join-Path $out "SwapCraft.exe")
    # the command line and MCP server (no window) sits beside the hub and updates with it
    Put (Join-Path $src "target\release\swapcraft-cli.exe") (Join-Path $out "swapcraft-cli.exe")
    # the drag-and-drop menu handler: File Explorer loads it, so it may be in use; like an exe, it can be renamed
    $dll = Join-Path $out "voidflare_dragdrop.dll"
    $oldDll = Join-Path $out "voidflare_dragdrop.old.dll"
    if (Test-Path $oldDll) { Remove-Item -LiteralPath $oldDll -Force -ErrorAction SilentlyContinue }
    if (Test-Path $dll) { try { Remove-Item -LiteralPath $dll -Force -ErrorAction Stop } catch { Move-Item -LiteralPath $dll -Destination $oldDll -Force } }
    Copy-Item (Join-Path $src "target\release\voidflare_dragdrop.dll") $dll -Force
    Write-Host "voidflare_dragdrop.dll"
    $engines = @{ "word-to-pdf" = "WordCraft"; "excel-to-pdf" = "GridCraft"; "slides-to-pdf" = "DeckCraft"; "pdf-tools" = "PdfCraft"; "dwg-to-pdf" = "CADCraft"; "image-converter" = "PhotoCraft"; "audio-converter" = "SoundCraft"; "video-converter" = "FilmCraft"; "ebook-converter" = "WordCraft" }
    $ids = Get-ChildItem (Join-Path $src "crates") -Directory -Filter "app-*" | ForEach-Object { $_.Name.Substring(4) }
    foreach ($id in $ids) {
        $dest = Join-Path $out "apps\$id"
        New-Item -ItemType Directory -Force $dest | Out-Null
        Copy-Item (Join-Path $src "target\release\$id.exe") $dest -Force
        Write-Host "apps\$id\$id.exe"
        # Each app carries its own engine, so nobody needs ArtCraft installed: the engine's command-line
        # program, its models and notices, and the MIT/Apache licence texts (all four are "MIT OR Apache-2.0").
        $engine = $engines[$id]
        if ($engine) {
            $lower = $engine.ToLower()
            # The engines come from src\engines\, filled by fetch-engines.ps1 with the versions pinned in engines.json
            # (official releases, hash-checked; CADCraft built from source with our patch, see -BuildCadcraft).
            # Until CADCraft has been built that way, the patched build kept in vendor\ is used.
            $installed = Join-Path $src "engines\$lower"
            if ($id -eq "dwg-to-pdf" -and -not (Test-Path (Join-Path $installed "$lower-cli.exe"))) { $installed = Join-Path $src "vendor\cadcraft" }
            $target = Join-Path $dest $lower
            if (-not (Test-Path (Join-Path $installed "$lower-cli.exe"))) { throw "$engine is not in $installed : run fetch-engines.ps1 first" }
            if (Test-Path $target) { Remove-Item -LiteralPath $target -Recurse -Force }
            New-Item -ItemType Directory -Force $target | Out-Null
            # everything except the engine's own window (<engine>.exe, not needed here)
            Get-ChildItem $installed -Force | Where-Object { $_.Name -ne "$lower.exe" } | ForEach-Object {
                Copy-Item $_.FullName $target -Recurse -Force
            }
            Copy-Item (Join-Path $src "licenses\$lower\*") $target -Force
            Write-Host "  + $lower\ ($engine)"
        }
    }
} finally {
    Pop-Location
}
