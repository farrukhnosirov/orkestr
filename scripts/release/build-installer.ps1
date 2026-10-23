<#
.SYNOPSIS
  Builds the Orkestr Windows installers (NSIS + MSI), signs the update
  artifacts and writes latest.json for the update server (ADR-0048).

.DESCRIPTION
  1. Reads the version from the workspace Cargo.toml ([workspace.package] version,
     the single version source; tauri.conf.json has no version of its own).
     -Version X.Y.Z bumps it first (Cargo.toml + root/desktop package.json).
  2. `tauri build` (runs the frontend build via beforeBuildCommand) for the
     requested bundles. Pre-release versions build NSIS only (MSI needs X.Y.Z).
  3. Signs every installer with the updater key (minisign, the format
     tauri-plugin-updater verifies; the trusted comment records the version)
     and verifies the signature against the public key in tauri.conf.json.
  4. Writes latest.json (+ SHA256SUMS.txt) and copies everything to
     dist-release\<version>\ (git-ignored).

  The private key never lives in the repository:
    -KeyPath       default %USERPROFILE%\.orkestr-tools\keys\updater.key
    -PasswordPath  default <KeyPath>.password
  Authenticode code signing is not configured yet (unsigned installers);
  pass -CertThumbprint to sign with a certificate from the Windows store.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\release\build-installer.ps1
  powershell -ExecutionPolicy Bypass -File scripts\release\build-installer.ps1 -Version 0.1.1 -Notes notes.md -NotesRu notes.ru.md -BaseUrl https://updates.orkestr.dev/download/0.1.1
#>
[CmdletBinding()]
param(
  [string]$Version = "",
  [string]$Notes = "",
  [string]$NotesRu = "",
  [ValidateSet("stable", "beta")][string]$Channel = "stable",
  # URL prefix of the installers on the update server; empty = URLs relative to latest.json
  # (a release folder / network share used as the local update channel).
  [string]$BaseUrl = "",
  [string]$Bundles = "nsis,msi",
  [string]$KeyPath = (Join-Path $env:USERPROFILE ".orkestr-tools\keys\updater.key"),
  [string]$PasswordPath = "",
  [string]$CertThumbprint = "",
  [string]$OutDir = "",
  # Extra Tauri config (JSON file) merged last, e.g. a test identifier for e2e install runs.
  [string]$ExtraConfig = "",
  [switch]$SkipFrontend
)

$ErrorActionPreference = "Stop"
$started = Get-Date
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if (-not $OutDir) { $OutDir = Join-Path $root "dist-release" }
if (-not $PasswordPath) { $PasswordPath = "$KeyPath.password" }
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

function Step($text) { Write-Host "==> $text" -ForegroundColor Cyan }
function Utf8NoBom($path, $text) { [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding $false)) }
function Run($exe, [string[]]$arguments) {
  & $exe @arguments
  if ($LASTEXITCODE -ne 0) { throw "$exe $($arguments -join ' ') failed with exit code $LASTEXITCODE" }
}

# ── version (single source: [workspace.package] in Cargo.toml) ─────────────
$cargoToml = Join-Path $root "Cargo.toml"
$cargoText = [System.IO.File]::ReadAllText($cargoToml)
$wsPattern = '(?ms)(^\[workspace\.package\]\s*\r?\nversion\s*=\s*")([^"]+)(")'
$m = [regex]::Match($cargoText, $wsPattern)
if (-not $m.Success) { throw "workspace version not found in Cargo.toml" }
$current = $m.Groups[2].Value
if ($Version -and $Version -ne $current) {
  if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') { throw "invalid version $Version" }
  Step "Bumping version $current -> $Version"
  Utf8NoBom $cargoToml ([regex]::Replace($cargoText, $wsPattern, "`${1}$Version`${3}", 1))
  foreach ($pj in @("package.json", "apps\desktop\package.json")) {
    $p = Join-Path $root $pj
    $t = [System.IO.File]::ReadAllText($p)
    Utf8NoBom $p ([regex]::Replace($t, '("version"\s*:\s*")[^"]+(")', "`${1}$Version`${2}", 1))
  }
} else {
  $Version = $current
}
$prerelease = $Version.Contains("-")
if ($prerelease -and $Bundles -match "msi") {
  Write-Warning "MSI needs a numeric X.Y.Z version; building NSIS only for $Version"
  $Bundles = "nsis"
}
Step "Orkestr $Version ($Channel), bundles: $Bundles"

if (-not (Test-Path $KeyPath)) { throw "updater private key not found: $KeyPath (see docs/adr/0048-updates-and-installer.md)" }
if (-not (Test-Path $PasswordPath)) { throw "updater key password file not found: $PasswordPath" }

# ── build ───────────────────────────────────────────────────────────────────
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root "target" }
$extra = @{}
if ($SkipFrontend) { $extra["build"] = @{ beforeBuildCommand = "" } }
if ($CertThumbprint) { $extra["bundle"] = @{ windows = @{ certificateThumbprint = $CertThumbprint; timestampUrl = "http://timestamp.digicert.com" } } }
$tauriArgs = @("run", "tauri", "-w", "@orkestr/desktop", "--", "build", "--bundles", $Bundles, "--ci")
if ($extra.Count -gt 0) {
  $cfg = Join-Path ([System.IO.Path]::GetTempPath()) "orkestr-release-config.json"
  Utf8NoBom $cfg ($extra | ConvertTo-Json -Depth 6)
  $tauriArgs += @("--config", $cfg)
}
if ($ExtraConfig) { $tauriArgs += @("--config", (Resolve-Path $ExtraConfig).Path) }
Step "tauri build (target dir: $targetDir)"
$buildStart = Get-Date
Push-Location $root
try { Run "npm.cmd" $tauriArgs } finally { Pop-Location }
$buildSecs = [int]((Get-Date) - $buildStart).TotalSeconds

$bundleDir = Join-Path $targetDir "release\bundle"
$installers = @()
$nsis = Get-ChildItem (Join-Path $bundleDir "nsis") -Filter "Orkestr_${Version}_*-setup.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($Bundles -match "nsis") {
  if (-not $nsis) { throw "NSIS installer for $Version not found under $bundleDir\nsis" }
  $installers += $nsis
}
$msis = @(Get-ChildItem (Join-Path $bundleDir "msi") -Filter "Orkestr_${Version}_*.msi" -ErrorAction SilentlyContinue)
if ($Bundles -match "msi") {
  if ($msis.Count -eq 0) { throw "MSI for $Version not found under $bundleDir\msi" }
  $installers += $msis
}

# ── sign + verify update artifacts ─────────────────────────────────────────
Step "Building the release tool"
Push-Location $root
try { Run "cargo" @("build", "-q", "-p", "orkestr-updater", "--bin", "orkestr-release", "-j", "4") } finally { Pop-Location }
$tool = Join-Path $targetDir "debug\orkestr-release.exe"
$conf = Get-Content (Join-Path $root "apps\desktop\src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json
$pubkey = $conf.plugins.updater.pubkey
foreach ($i in $installers) {
  Step "Signing $($i.Name)"
  Run $tool @("sign", "--key", $KeyPath, "--password-file", $PasswordPath, "--version", $Version, $i.FullName) | Out-Null
  Run $tool @("verify", "--pubkey", $pubkey, "--version", $Version, $i.FullName)
}

# ── release folder + latest.json ───────────────────────────────────────────
$dest = Join-Path $OutDir $Version
New-Item -ItemType Directory -Force $dest | Out-Null
foreach ($i in $installers) {
  Copy-Item $i.FullName $dest -Force
  Copy-Item "$($i.FullName).sig" $dest -Force
}
$platforms = @()
if ($nsis -and ($Bundles -match "nsis")) {
  $n = Join-Path $dest $nsis.Name
  $platforms += @("--platform", "windows-x86_64=$n", "--platform", "windows-x86_64-nsis=$n")
}
$msiEn = $msis | Where-Object { $_.Name -like "*en-US*" } | Select-Object -First 1
if ($msiEn -and ($Bundles -match "msi")) { $platforms += @("--platform", "windows-x86_64-msi=$(Join-Path $dest $msiEn.Name)") }
$manifestArgs = @("manifest", "--version", $Version, "--channel", $Channel, "--out", (Join-Path $dest "latest.json")) + $platforms
if ($BaseUrl) { $manifestArgs += @("--base-url", $BaseUrl) }
if ($Notes) { $manifestArgs += @("--notes-file", (Resolve-Path $Notes).Path) }
if ($NotesRu) { $manifestArgs += @("--notes-ru-file", (Resolve-Path $NotesRu).Path) }
Step "Writing latest.json"
Run $tool $manifestArgs | Out-Null

$sums = Get-ChildItem $dest -File | Where-Object { $_.Name -ne "SHA256SUMS.txt" } | ForEach-Object {
  "{0}  {1}" -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower(), $_.Name
}
Utf8NoBom (Join-Path $dest "SHA256SUMS.txt") (($sums -join "`n") + "`n")

Step "Done in $([int]((Get-Date) - $started).TotalSeconds) s (tauri build: $buildSecs s)"
Get-ChildItem $dest -File | ForEach-Object { "{0,-48} {1,10:N1} MB" -f $_.Name, ($_.Length / 1MB) } | Write-Host
Write-Host "Release folder: $dest"
