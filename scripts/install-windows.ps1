# Builds Ghostwrite and installs it for this user, with a Start menu entry.
# It needs Rust with the MSVC toolchain, cmake, and LLVM (for llama.cpp's bindings).
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# The plugins that come with it, as scripts/build-plugins.sh puts them: each zip plugins.lock
# pins (`<name> <tag> <url> <sha256>`), downloaded once into target\plugin-downloads, checked,
# and unzipped into target\bundled-plugins.
$bundled = "target\bundled-plugins"
$downloads = "target\plugin-downloads"
Remove-Item -Recurse -Force $bundled -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $bundled, $downloads | Out-Null
foreach ($line in Get-Content "plugins.lock") {
    if ($line -match '^\s*(#|$)') { continue }
    $name, $tag, $url, $sum = $line -split '\s+'
    $zip = Join-Path $downloads "$name-$tag.zip"
    if (-not (Test-Path $zip) -or (Get-FileHash $zip -Algorithm SHA256).Hash -ne $sum) {
        Invoke-WebRequest -Uri $url -OutFile "$zip.part"
        if ((Get-FileHash "$zip.part" -Algorithm SHA256).Hash -ne $sum) {
            Remove-Item "$zip.part"
            throw "$url isn't the $name plugins.lock pins"
        }
        Move-Item -Force "$zip.part" $zip
    }
    Expand-Archive -Path $zip -DestinationPath $bundled -Force
    if (-not (Test-Path (Join-Path $bundled "$name\plugin.json"))) { throw "$zip has no $name\plugin.json" }
}

$dir = Join-Path $env:LOCALAPPDATA "Programs\Ghostwrite"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item "target\release\ghostwrite.exe" $dir -Force
Copy-Item "assets\icon\ghostwrite.ico" $dir -Force
Copy-Item "LICENSE" $dir -Force
Remove-Item -Recurse -Force (Join-Path $dir "plugins") -ErrorAction SilentlyContinue
Copy-Item $bundled (Join-Path $dir "plugins") -Recurse -Force

$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath("Programs")) "Ghostwrite.lnk"))
$link.TargetPath = Join-Path $dir "ghostwrite.exe"
$link.IconLocation = Join-Path $dir "ghostwrite.ico"
$link.Save()
Write-Host "Installed $dir\ghostwrite.exe"
