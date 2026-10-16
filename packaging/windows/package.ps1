# Builds dist/Sonkore-windows-x64.zip (the same name for every version, so
# releases/latest/download/ links never change; it unzips to a Sonkore folder)
# from a Release build: Qt and
# its QML modules (windeployqt), the MSVC runtime (app-local, so nothing has to
# be installed first), the audio-only FFmpeg DLLs and every license.
#   packaging/windows/package.ps1 -Build <build dir> -FFmpeg <ffmpeg prefix> [-Out dist]
# Run from a Visual Studio developer shell (VCToolsRedistDir) with Qt's bin/
# on PATH.
param(
    [Parameter(Mandatory = $true)][string]$Build,
    [Parameter(Mandatory = $true)][string]$FFmpeg,
    [string]$Out = "dist"
)
$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot/../.."
$version = (Select-String -Path "$root/CMakeLists.txt" -Pattern '^\s*VERSION ([0-9.]+)$').Matches[0].Groups[1].Value
$name = "Sonkore-windows-x64"
$stage = Join-Path $Out "Sonkore"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item "$Build/bin/Sonkore.exe" $stage
& windeployqt --release --qmldir "$root/qml" --no-translations --no-system-d3d-compiler --no-opengl-sw "$stage/Sonkore.exe"
if ($LASTEXITCODE -ne 0) { throw "windeployqt failed" }
# Image formats: covers are JPEG, PNG, WebP, GIF or BMP (PNG and BMP are built
# into Qt), icons SVG. Qt Quick instantiates every image plugin present when the
# first picture loads, so the others (TIFF, ICNS, ICO, TGA, WBMP) are left out.
Get-ChildItem "$stage/imageformats" -Filter *.dll | Where-Object { $_.BaseName -notin @("qsvg", "qjpeg", "qwebp", "qgif") } | Remove-Item

# MSVC runtime next to the exe (Microsoft's app-local deployment).
$crt = Get-ChildItem -Directory "$env:VCToolsRedistDir/x64" -Filter "Microsoft.VC*.CRT" | Select-Object -First 1
if (-not $crt) { throw "MSVC runtime not found under VCToolsRedistDir=$env:VCToolsRedistDir" }
Copy-Item "$($crt.FullName)/*.dll" $stage

# FFmpeg, opened on demand (LoadLibrary "avformat-NN.dll").
Copy-Item "$FFmpeg/bin/*.dll" $stage

$docs = New-Item -ItemType Directory -Force -Path "$stage/licenses"
Copy-Item "$root/LICENSE" "$stage/LICENSE.txt"
Copy-Item "$root/THIRD-PARTY.md" $docs
Copy-Item "$root/licenses/*" $docs
Copy-Item "$FFmpeg/COPYING.LGPLv2.1" "$docs/FFmpeg-COPYING.LGPLv2.1.txt"
Copy-Item "$FFmpeg/BUILD.txt" "$docs/FFmpeg-BUILD.txt"
Get-ChildItem -Directory "$root/fonts" | ForEach-Object {
    Copy-Item "$($_.FullName)/OFL.txt" "$docs/font-$($_.Name)-OFL.txt"
}
$qtVersion = (& qtpaths --qt-query QT_VERSION)
@"
Sonkore $version for Windows x64 - bundled libraries

Qt $qtVersion  LGPL-3.0 (licenses/LGPL-3.0.txt)
  Unmodified binaries from the Qt online installer. Source:
  https://download.qt.io/official_releases/qt/

FFmpeg (avformat, avcodec, avutil, swresample DLLs)  LGPL-2.1-or-later
  See licenses/FFmpeg-BUILD.txt for the exact source and configuration.

Microsoft Visual C++ runtime (msvcp140, vcruntime140...)  redistributed under
  the Visual Studio license terms for app-local deployment.

The DLLs can be replaced with compatible builds.
"@ | Set-Content -Encoding utf8 "$docs/BUNDLED.txt"

$zip = Join-Path $Out "$name.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $stage -DestinationPath $zip -CompressionLevel Optimal
$hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
"$hash  $name.zip" | Set-Content -Encoding ascii "$zip.sha256"
Write-Host "Package: $zip"
