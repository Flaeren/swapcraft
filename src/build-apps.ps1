# Builds the hub and the Rust apps in release and puts the products in ..\portable, the only folder
# that needs zipping to try voidflare on another PC:
#   ..\portable\voidflare.exe          the hub
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
    $hub = Join-Path $out "voidflare.exe"
    $old = Join-Path $out "voidflare.old.exe"
    if (Test-Path $old) { Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue }
    # a running exe cannot be overwritten but can be renamed
    if (Test-Path $hub) { try { Remove-Item -LiteralPath $hub -Force -ErrorAction Stop } catch { Move-Item -LiteralPath $hub -Destination $old -Force } }
    Copy-Item (Join-Path $src "target\release\voidflare-hub.exe") $hub -Force
    Write-Host "voidflare.exe"
    $engines = @{ "word-to-pdf" = "WordCraft"; "excel-to-pdf" = "GridCraft"; "slides-to-pdf" = "DeckCraft"; "pdf-tools" = "PdfCraft"; "dwg-to-pdf" = "CADCraft" }
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
            # CADCraft is our own patched build (vendor\), the others are the installed ArtCraft engines
            $installed = if ($id -eq "dwg-to-pdf") { Join-Path $src "vendor\cadcraft" } else { Join-Path ${env:ProgramFiles} $engine }
            $target = Join-Path $dest $lower
            if (-not (Test-Path (Join-Path $installed "$lower-cli.exe"))) { throw "$engine is not installed: cannot bundle it" }
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
