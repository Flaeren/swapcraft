# Gets the conversion engines the apps bundle, from their official GitHub releases, at the versions pinned in
# engines.json, and unpacks them into src\engines\<name>\ . Every download is checked against the pinned SHA-256 and
# refused when it differs, so a build always uses exactly the engines the repository names.
#
#   powershell -File fetch-engines.ps1                 the four engines that ship as binaries
#   powershell -File fetch-engines.ps1 -BuildCadcraft  also builds CADCraft from source with our patch (needs git and Rust)
#
# The cache in src\engines\.cache keeps the ZIPs, so a second run downloads nothing.
param([switch]$BuildCadcraft)
$ErrorActionPreference = "Stop"
$src = Split-Path -Parent $MyInvocation.MyCommand.Path
$cfg = Get-Content (Join-Path $src "engines.json") -Raw | ConvertFrom-Json
$root = Join-Path $src "engines"
$cache = Join-Path $root ".cache"
New-Item -ItemType Directory -Force $cache | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Get-Verified($url, $file, $sha256) {
    if (Test-Path $file) {
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLower() -eq $sha256) { return }
        Remove-Item -LiteralPath $file -Force
    }
    Write-Host "  downloading $(Split-Path -Leaf $file)"
    Invoke-WebRequest -Uri $url -OutFile "$file.part" -UseBasicParsing
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath "$file.part").Hash.ToLower()
    if ($actual -ne $sha256) {
        Remove-Item -LiteralPath "$file.part" -Force
        throw "SHA-256 of $(Split-Path -Leaf $file) is $actual, the pinned one is ${sha256}; refusing it"
    }
    Move-Item -LiteralPath "$file.part" -Destination $file -Force
}

foreach ($e in $cfg.engines) {
    Write-Host "$($e.name) $($e.tag)"
    $zip = Join-Path $cache $e.asset
    Get-Verified "https://github.com/$($e.repo)/releases/download/$($e.tag)/$($e.asset)" $zip $e.sha256
    $dest = Join-Path $root $e.name
    if (Test-Path $dest) { Remove-Item -LiteralPath $dest -Recurse -Force }
    New-Item -ItemType Directory -Force $dest | Out-Null
    # The portable ZIP holds one folder: the engine's window (<name>.exe), its command-line program, licences, notices.
    # The apps need everything except the window.
    $archive = [System.IO.Compression.ZipFile]::OpenRead($zip)
    try {
        foreach ($entry in $archive.Entries) {
            # not the window, and not the portable-mode marker or the window's README: the apps were tried without them
            if ($entry.Name -eq "" -or $entry.Name -in @("$($e.name).exe", "portable.txt", "README.md")) { continue }
            $relative = ($entry.FullName -split "/", 2)[1]
            if (-not $relative) { continue }
            $target = Join-Path $dest ($relative -replace "/", "\")
            $full = [System.IO.Path]::GetFullPath($target)
            if (-not $full.StartsWith([System.IO.Path]::GetFullPath($dest))) { throw "unsafe path in $($e.asset): $($entry.FullName)" }
            New-Item -ItemType Directory -Force (Split-Path -Parent $full) | Out-Null
            [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $full, $true)
        }
    } finally { $archive.Dispose() }
    if (-not (Test-Path (Join-Path $dest "$($e.name)-cli.exe"))) { throw "$($e.asset) has no $($e.name)-cli.exe" }
    # our copies of the licence texts, as before
    $own = Join-Path $src "licenses\$($e.name)"
    if (Test-Path $own) { Copy-Item (Join-Path $own "*") $dest -Force }
}

if ($BuildCadcraft) {
    $c = $cfg.cadcraft
    Write-Host "cadcraft $($c.tag) from source, with $($c.patch)"
    $work = Join-Path $root ".cadcraft-src"
    if (Test-Path $work) { Remove-Item -LiteralPath $work -Recurse -Force }
    git clone --quiet --depth 1 --branch $c.tag "https://github.com/$($c.repo).git" $work
    Push-Location $work
    try {
        $head = (git rev-parse HEAD).Trim()
        if ($head -ne $c.commit) { throw "tag $($c.tag) points at $head, the pinned commit is $($c.commit); refusing it" }
        git apply (Join-Path $src $c.patch)
        if ($LASTEXITCODE -ne 0) { throw "the patch does not apply to $($c.tag)" }
        # the C runtime goes into the exe, so a clean PC needs no Visual C++ redistributable
        $env:RUSTFLAGS = "-C target-feature=+crt-static"
        & (Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe") build --release -p $c.package
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
        $dest = Join-Path $root "cadcraft"
        if (Test-Path $dest) { Remove-Item -LiteralPath $dest -Recurse -Force }
        New-Item -ItemType Directory -Force $dest | Out-Null
        Copy-Item (Join-Path $work "target\release\cadcraft-cli.exe") $dest
        foreach ($f in "LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "ATTRIBUTION.md") { Copy-Item (Join-Path $work $f) $dest }
    } finally { Pop-Location }
    Remove-Item -LiteralPath $work -Recurse -Force
}
Write-Host "engines ready in $root"
