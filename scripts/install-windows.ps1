# Builds Ghostwrite and installs it for this user, with a Start menu entry.
# It needs Rust with the MSVC toolchain, cmake, and LLVM (for llama.cpp's bindings).
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# The plugins that come with it, as scripts/build-plugins.sh builds them: each one's code,
# the grammars its grammars.txt lists (downloaded once, and checked), and its other files.
rustup target add wasm32-wasip2
$bundled = "target\bundled-plugins"
$grammars = "target\grammars"
Remove-Item -Recurse -Force $bundled -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $grammars | Out-Null
foreach ($source in Get-ChildItem -Directory plugins) {
    $plugin = $source.Name
    $out = Join-Path $bundled $plugin
    New-Item -ItemType Directory -Force -Path $out | Out-Null
    if (Test-Path (Join-Path $source.FullName "Cargo.toml")) {
        cargo build --release --target wasm32-wasip2 --manifest-path "plugins\$plugin\Cargo.toml" --target-dir target\plugin-builds
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        Copy-Item "target\plugin-builds\wasm32-wasip2\release\ghostwrite_${plugin}_plugin.wasm" (Join-Path $out "$plugin.wasm") -Force
    }
    $list = Join-Path $source.FullName "grammars.txt"
    if (Test-Path $list) {
        foreach ($line in Get-Content $list) {
            if ($line -match '^\s*(#|$)') { continue }
            $sum, $url = $line -split '\s+', 2
            $file = Join-Path $grammars ($url.Split('/')[-1])
            if (-not (Test-Path $file) -or (Get-FileHash $file -Algorithm SHA256).Hash -ne $sum) {
                Invoke-WebRequest -Uri $url -OutFile "$file.part"
                if ((Get-FileHash "$file.part" -Algorithm SHA256).Hash -ne $sum) {
                    Remove-Item "$file.part"
                    throw "$url isn't the grammar $list says"
                }
                Move-Item -Force "$file.part" $file
            }
            Copy-Item $file $out -Force
        }
    }
    Get-ChildItem $source.FullName | Where-Object { $_.Name -notin "Cargo.toml", "Cargo.lock", "src", "grammars.txt" } | Copy-Item -Destination $out -Recurse -Force
}

$dir = Join-Path $env:LOCALAPPDATA "Programs\Ghostwrite"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item "target\release\ghostwrite.exe" $dir -Force
Copy-Item "assets\icon\ghostwrite.ico" $dir -Force
Remove-Item -Recurse -Force (Join-Path $dir "plugins") -ErrorAction SilentlyContinue
Copy-Item $bundled (Join-Path $dir "plugins") -Recurse -Force

$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath("Programs")) "Ghostwrite.lnk"))
$link.TargetPath = Join-Path $dir "ghostwrite.exe"
$link.IconLocation = Join-Path $dir "ghostwrite.ico"
$link.Save()
Write-Host "Installed $dir\ghostwrite.exe"
