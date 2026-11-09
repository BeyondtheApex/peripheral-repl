# Package the matching EXE/DLL produced by ./build.ps1 -Release.
param(
    [string]$Destination=(Join-Path $env:LOCALAPPDATA 'peripheral-repl-dist/ui'),
    [string]$PluginSource,
    [string]$DefaultReplPolicy
)
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
# Copy only manifest-declared plugin files; never bundle sessions, grants or user settings.
if ($PluginSource) {
    $pluginRoot=(Resolve-Path -LiteralPath $PluginSource).Path
    foreach($directory in Get-ChildItem -LiteralPath $pluginRoot -Directory) {
        $manifestPath=Join-Path $directory.FullName 'plugin.json'
        if(!(Test-Path -LiteralPath $manifestPath)){continue}
        $plugin=Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        if($plugin.id -ne $directory.Name -or $plugin.id -notmatch '^[a-zA-Z0-9_-]{1,48}$'){throw 'Invalid plugin directory or ID'}
        $target=Join-Path $Destination "plugins/$($plugin.id)"
        [void](New-Item -ItemType Directory -Path $target -Force)
        $inventory=@('plugin.json')+@($plugin.files)
        if($plugin.resources){$inventory+=@($plugin.resources.PSObject.Properties.Value)}
        foreach($relative in $inventory) {
            if(!$relative -or [IO.Path]::IsPathRooted($relative) -or $relative -match '(^|[\/])\.\.([\/]|$)'){throw 'Plugin path escapes package'}
            $original=Join-Path $directory.FullName $relative
            # Reject links at every level instead of following external content.
            $entry=Get-Item -LiteralPath $original
            if($entry.PSIsContainer){throw 'Plugin inventory entries must be files'}
            while($entry -and $entry.FullName -ne $pluginRoot) {
                if($entry.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Plugin package cannot contain links'}
                $entry=Get-Item -LiteralPath (Split-Path -Parent $entry.FullName)
            }
            $resolved=(Resolve-Path -LiteralPath $original).Path
            if(!$resolved.StartsWith($directory.FullName+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Plugin path escapes package'}
            $output=Join-Path $target $relative
            [void](New-Item -ItemType Directory -Path (Split-Path -Parent $output) -Force)
            Copy-Item -LiteralPath $original -Destination $output
        }
    }
}
if($DefaultReplPolicy) {
    $policy=Get-Content -LiteralPath $DefaultReplPolicy -Raw | ConvertFrom-Json
    if(!$policy){throw 'Invalid default REPL policy'}
    [void](New-Item -ItemType Directory -Path (Join-Path $Destination '.peripheral'))
    Copy-Item -LiteralPath $DefaultReplPolicy -Destination (Join-Path $Destination '.peripheral/policy.json')
}
# ZipFile includes hidden/dot directories; Compress-Archive can omit them.
[IO.Compression.ZipFile]::CreateFromDirectory($Destination,"$Destination.zip")
(Get-FileHash -LiteralPath "$Destination.zip" -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + [IO.Path]::GetFileName("$Destination.zip") | Set-Content -LiteralPath "$Destination.zip.sha256" -Encoding ascii
$manifest|ConvertTo-Json -Depth 6
