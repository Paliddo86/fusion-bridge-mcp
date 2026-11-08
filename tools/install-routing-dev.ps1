param(
    [Parameter(Mandatory=$true)][string]$Cache,
    [Parameter(Mandatory=$true)][string]$Staged,
    [string]$Version='1.10.0-dev.routing.4'
)
$ErrorActionPreference='Stop'
if (!(Test-Path "$cache/.dev-pin")) { throw 'Fusion bridge is not dev-pinned' }
$backup="$cache/routing-backup-$(Get-Date -Format yyyyMMdd-HHmmss)"
if (Test-Path $backup) { throw 'Backup already exists; inspect before retrying' }
New-Item -ItemType Directory -Path $backup | Out-Null
foreach ($name in @('adom-fusion-bridge.exe','BRIDGE_VERSION','bridge.json')) { Copy-Item "$cache/$name" "$backup/$name" }
Copy-Item $staged "$cache/adom-fusion-bridge.exe" -Force
$manifest=Get-Content "$cache/bridge.json" -Raw | ConvertFrom-Json
$manifest.version=$Version
[IO.File]::WriteAllText("$cache/bridge.json",($manifest | ConvertTo-Json -Depth 30),[Text.UTF8Encoding]::new($false))
[IO.File]::WriteAllText("$cache/BRIDGE_VERSION","$Version`n",[Text.UTF8Encoding]::new($false))
$installed=(Get-FileHash "$cache/adom-fusion-bridge.exe" -Algorithm SHA256).Hash
$expected=(Get-FileHash $staged -Algorithm SHA256).Hash
if ($installed -ne $expected) { throw 'Installed hash mismatch' }
@{backup=$backup;sha256=$installed;version=$manifest.version} | ConvertTo-Json
exit 0
