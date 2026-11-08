param(
    [Parameter(Mandatory=$true)][string]$Cache,
    [Parameter(Mandatory=$true)][string]$Staged,
    [Parameter(Mandatory=$true)][string]$Version
)
# Invoked only after bridge_dev_mode(on:true), bridge_pause and staged hash checks.
# The bridge's normal startup installs the bundled add-in into Fusion's scan paths.
$ErrorActionPreference = 'Stop'
if (!(Test-Path -LiteralPath (Join-Path $Cache '.dev-pin'))) { throw 'Fusion bridge is not dev-pinned' }
$manifestPath = Join-Path $Staged 'mechanical-dev-manifest.json'
$payload = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($payload.version -ne $Version) { throw 'Staged version mismatch' }
$cacheRoot = [IO.Path]::GetFullPath($Cache).TrimEnd('\') + '\'
$stagedRoot = [IO.Path]::GetFullPath($Staged).TrimEnd('\') + '\'
foreach ($file in $payload.files) {
    $source = [IO.Path]::GetFullPath((Join-Path $Staged $file.path))
    $destination = [IO.Path]::GetFullPath((Join-Path $Cache $file.path))
    if (!$source.StartsWith($stagedRoot, [StringComparison]::OrdinalIgnoreCase) -or !$destination.StartsWith($cacheRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Payload path escapes its directory' }
    if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ne $file.sha256) { throw "Staged hash mismatch: $($file.path)" }
}
$backup = Join-Path $Cache ('mechanical-backup-' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff'))
New-Item -ItemType Directory -Path $backup | Out-Null
$created = @()
foreach ($file in $payload.files) {
    $destination = Join-Path $Cache $file.path
    if (Test-Path -LiteralPath $destination) {
        $saved = Join-Path $backup $file.path
        New-Item -ItemType Directory -Force -Path (Split-Path $saved) | Out-Null
        Copy-Item -LiteralPath $destination -Destination $saved
    } else { $created += $file.path }
}
$created | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $backup 'new-paths.json') -Encoding UTF8
# Only this bridge executable; never stop Fusion or other bridges here.
$exe = Join-Path $Cache 'adom-fusion-bridge.exe'
$stopped = @()
Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath -ieq $exe } | ForEach-Object {
    $stopped += $_.ProcessId
    Stop-Process -Id $_.ProcessId -Force -ErrorAction Stop
}
try {
    foreach ($file in $payload.files) {
        $destination = Join-Path $Cache $file.path
        New-Item -ItemType Directory -Force -Path (Split-Path $destination) | Out-Null
        Copy-Item -LiteralPath (Join-Path $Staged $file.path) -Destination $destination -Force
        if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -ne $file.sha256) { throw "Installed hash mismatch: $($file.path)" }
    }
} catch {
    $failure = $_
    foreach ($file in $payload.files) {
        $saved = Join-Path $backup $file.path
        $destination = Join-Path $Cache $file.path
        if (Test-Path -LiteralPath $saved) { Copy-Item -LiteralPath $saved -Destination $destination -Force }
        elseif ($created -contains $file.path) { Remove-Item -LiteralPath $destination -Force -ErrorAction SilentlyContinue }
    }
    throw $failure
}
@{success=$true;version=$Version;backup=$backup;fileCount=$payload.files.Count;stoppedPids=$stopped;binarySha256=(Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash} | ConvertTo-Json
