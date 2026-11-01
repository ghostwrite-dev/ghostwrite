# Builds Ghostwrite and installs it for this user, with a Start menu entry.
# It needs Rust with the MSVC toolchain, cmake, and LLVM (for llama.cpp's bindings).
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
rustup target add wasm32-wasip2
foreach ($plugin in "typst", "pdf", "image") {
    cargo build --release --target wasm32-wasip2 --manifest-path "plugins\$plugin\Cargo.toml" --target-dir target\plugin-builds
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$dir = Join-Path $env:LOCALAPPDATA "Programs\Ghostwrite"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item "target\release\ghostwrite.exe" $dir -Force
foreach ($plugin in "typst", "pdf", "image") {
    $to = Join-Path $dir "plugins\$plugin"
    New-Item -ItemType Directory -Force -Path $to | Out-Null
    Copy-Item "target\plugin-builds\wasm32-wasip2\release\ghostwrite_${plugin}_plugin.wasm" (Join-Path $to "$plugin.wasm") -Force
    Copy-Item "plugins\$plugin\plugin.json" $to -Force
}

$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath("Programs")) "Ghostwrite.lnk"))
$link.TargetPath = Join-Path $dir "ghostwrite.exe"
$link.Save()
Write-Host "Installed $dir\ghostwrite.exe"
