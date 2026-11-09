# Requires stable Rust (MSVC), Visual Studio C++ tools, Windows SDK and CMake.
param([switch]$Release, [switch]$Run, [string]$NativeBuildDirectory=(Join-Path $env:LOCALAPPDATA 'peripheral-repl-build/native-ui'))
$ErrorActionPreference = 'Stop'
$projectRoot = $PSScriptRoot
$env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA 'peripheral-repl-build/ui'
$configuration = if ($Release) { 'Release' } else { 'Debug' }
$nativeBuild = $NativeBuildDirectory
$nativeDll = Join-Path $nativeBuild "$configuration/peripheral-taskbar-hook.dll"
# Use CMake's installed Visual Studio generator; do not pin a local VS version.
& cmake -S (Join-Path $projectRoot 'ui/native') -B $nativeBuild -A x64
if ($LASTEXITCODE) { exit $LASTEXITCODE }
& cmake --build $nativeBuild --config $configuration --target peripheral-taskbar-hook
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$env:PERIPHERAL_NATIVE_SHA256 = (Get-FileHash -LiteralPath $nativeDll -Algorithm SHA256).Hash.ToLowerInvariant()
$header = Get-Content -LiteralPath (Join-Path $nativeBuild 'native_build.h') -Raw
if ($header -notmatch 'PERIPHERAL_NATIVE_BUILD "([0-9a-f]{64})"') { throw 'Native build identity missing' }
$env:PERIPHERAL_NATIVE_BUILD = $Matches[1]
$output = Join-Path $env:CARGO_TARGET_DIR $(if($Release){'release'}else{'debug'})
[void](New-Item -ItemType Directory -Force -Path $output)
$destination = Join-Path $output 'peripheral-taskbar-hook.dll'
if (!(Test-Path -LiteralPath $destination) -or (Get-FileHash -LiteralPath $destination).Hash.ToLowerInvariant() -ne $env:PERIPHERAL_NATIVE_SHA256) {
    # Never write through a loaded DLL. A locked destination fails with a clear error.
    try {Copy-Item -LiteralPath $nativeDll -Destination $destination -Force} catch {throw "Native DLL is in use at $destination. Close the UI; if Explorer loaded it, update after a normal sign-out/restart. $($_.Exception.Message)"}
}
$verb = 'build'
$buildArgs = @($verb, '--locked', '--manifest-path', (Join-Path $projectRoot 'ui/Cargo.toml'))
if ($Release) { $buildArgs += '--release' }
& cargo @buildArgs
if($LASTEXITCODE){exit $LASTEXITCODE}
# GPUI already embeds RT_MANIFEST #1. Merge the XAML Islands compatibility
# declaration into that resource instead of linking a duplicate manifest.
$sdk=Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Microsoft SDKs\Windows\v10.0'
$mt=Get-ChildItem -LiteralPath (Join-Path $sdk.InstallationFolder 'bin') -Directory | Sort-Object Name -Descending | ForEach-Object {Join-Path $_.FullName 'x64/mt.exe'} | Where-Object {Test-Path -LiteralPath $_} | Select-Object -First 1
if(!$mt){throw 'Windows SDK mt.exe is required to merge the UI compatibility manifest'}
$exe=Join-Path $output 'peripheral-ui.exe'
& $mt -nologo -manifest (Join-Path $projectRoot 'ui/native/host.manifest') "-inputresource:$exe;#1" "-outputresource:$exe;#1"
if($LASTEXITCODE){exit $LASTEXITCODE}
if($Run){& $exe;exit $LASTEXITCODE}
exit 0
