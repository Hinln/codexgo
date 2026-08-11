param(
    [string]$Configuration = "release"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$dist = Join-Path $root "dist"
$manifest = Join-Path $root "src-tauri\Cargo.toml"
$target = Join-Path $root "src-tauri\target\$Configuration\CodexGo.exe"

$env:CARGO_BUILD_JOBS = "1"

Push-Location $root
try {
    pnpm install --frozen-lockfile
    pnpm build:web
    cargo test --manifest-path $manifest
    cargo build --$Configuration --manifest-path $manifest

    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    Copy-Item -LiteralPath $target -Destination (Join-Path $dist "CodexGo.exe") -Force

    $hash = Get-FileHash -Algorithm SHA256 (Join-Path $dist "CodexGo.exe")
    "$($hash.Hash)  CodexGo.exe" |
        Set-Content -Encoding ascii (Join-Path $dist "SHA256SUMS.txt")

    Copy-Item -LiteralPath (Join-Path $PSScriptRoot "release-readme.txt") `
        -Destination (Join-Path $dist "README.txt") -Force
}
finally {
    Pop-Location
}
