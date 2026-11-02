$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$binary = Join-Path $root 'target/release/grit.exe'
$readme = Join-Path $root 'README.md'
$license = Join-Path $root 'LICENSE'
$dist = Join-Path $root 'dist'
$archive = Join-Path $dist 'grit-windows-x86_64.zip'

if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "Release binary not found: $binary"
}
foreach ($file in @($readme, $license)) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "Required package file not found: $file"
    }
}

New-Item -ItemType Directory -Path $dist -Force | Out-Null
Compress-Archive -LiteralPath @($binary, $readme, $license) -DestinationPath $archive -Force
Write-Host "Created $archive"
