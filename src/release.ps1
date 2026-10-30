# Packs ..\portable into the files a GitHub release carries, in ..\release\ :
#   voidflare-<version>-windows.zip   the portable folder (hub + apps + engines)
#   SHA256SUMS.txt                    "<hash>  <file name>", the format the hub checks downloads against
# It does NOT publish anything. To publish (after trying the ZIP on another PC):
#   gh release create v<version> ..\release\* --title "voidflare <version>" --notes-file <notes>
# Usage: powershell -File release.ps1   (run build-apps.ps1 first)
$ErrorActionPreference = "Stop"
$src = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Split-Path -Parent $src
$portable = Join-Path $root "portable"
$out = Join-Path $root "release"
if (-not (Test-Path (Join-Path $portable "voidflare.exe"))) { throw "portable\voidflare.exe is missing: run build-apps.ps1 first" }
$version = (Select-String -Path (Join-Path $src "Cargo.toml") -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
New-Item -ItemType Directory -Force $out | Out-Null
$zip = Join-Path $out "voidflare-$version-windows.zip"
if (Test-Path $zip) { Remove-Item -LiteralPath $zip -Force }
Compress-Archive -Path (Join-Path $portable "*") -DestinationPath $zip -CompressionLevel Optimal
$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLower()
# no BOM, LF line ending: sha256sum -c and the hub both read this
[System.IO.File]::WriteAllText((Join-Path $out "SHA256SUMS.txt"), "$hash  $(Split-Path -Leaf $zip)`n", (New-Object System.Text.UTF8Encoding($false)))
Write-Host ("{0}  {1:N0} MB" -f (Split-Path -Leaf $zip), ((Get-Item $zip).Length / 1MB))
Write-Host "SHA256SUMS.txt  $hash"
