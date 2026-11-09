# Package the matching EXE/DLL produced by ./build.ps1 -Release.
param([string]$Destination=(Join-Path $env:LOCALAPPDATA 'peripheral-repl-dist/ui'))
$ErrorActionPreference='Stop'
$source=Join-Path $env:LOCALAPPDATA 'peripheral-repl-build/ui/release'
$files=@('peripheral-ui.exe','peripheral-taskbar-hook.dll')
foreach($file in $files){if(!(Test-Path -LiteralPath (Join-Path $source $file))){throw "Missing release artifact: $file"}}
if(Test-Path -LiteralPath $Destination){throw 'Choose a new empty package directory; never overwrite a potentially loaded DLL.'}
[void](New-Item -ItemType Directory -Path $Destination)
foreach($file in $files){Copy-Item -LiteralPath (Join-Path $source $file) -Destination (Join-Path $Destination $file)}
$items=@(foreach($file in $files){$path=Join-Path $Destination $file;$signature=Get-AuthenticodeSignature -LiteralPath $path;[ordered]@{name=$file;sha256=(Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant();bytes=(Get-Item -LiteralPath $path).Length;signature_status=$signature.Status.ToString()}})
$manifest=[ordered]@{format=1;created=(Get-Date).ToString('o');architecture='x64';kind='unsigned development package';files=$items}
$manifest|ConvertTo-Json -Depth 6|Set-Content -Encoding utf8 (Join-Path $Destination 'release-manifest.json')
foreach($file in @('LICENSE','THIRD_PARTY_NOTICES.txt')) { Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination $Destination }
Compress-Archive -Path (Join-Path $Destination '*') -DestinationPath "$Destination.zip"
(Get-FileHash -LiteralPath "$Destination.zip" -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + [IO.Path]::GetFileName("$Destination.zip") | Set-Content -LiteralPath "$Destination.zip.sha256" -Encoding ascii
$manifest|ConvertTo-Json -Depth 6
