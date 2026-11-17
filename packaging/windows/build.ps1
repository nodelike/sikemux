# Packs release builds of sikemux.exe and sikemux-host.exe into the installer. resources.ps1 wrote the icon they and the installer carry.
param(
  [Parameter(Mandatory)][ValidateSet("x64", "arm64")][string]$Arch,
  [Parameter(Mandatory)][string]$Version,
  [Parameter(Mandatory)][string]$Binaries,
  [Parameter(Mandatory)][string]$Resources,
  [Parameter(Mandatory)][string]$Out
)
$ErrorActionPreference = "Stop"
$Here = $PSScriptRoot

if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)(-nightly\.(\d+))?$') {
  throw "$Version is neither a stable nor a nightly version"
}
$Build = if ($Matches[5]) { $Matches[5] } else { "0" }
$Numeric = "$($Matches[1]).$($Matches[2]).$($Matches[3]).$Build"

$Stage = Join-Path $env:RUNNER_TEMP "sikemux-stage-$Arch"
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $Stage, $Out | Out-Null
Copy-Item "$Resources\sikemux.ico" $Stage

foreach ($Name in "sikemux.exe", "sikemux-host.exe") {
  $Built = Join-Path $Binaries $Name
  if (-not (Test-Path $Built)) { throw "$Built was not built" }
  Copy-Item $Built $Stage
}

$Iscc = @(
  (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source,
  "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
  "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
  "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $Iscc) {
  choco install innosetup --no-progress -y | Out-Host
  $Iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
}
& $Iscc /Q "/DVersion=$Version" "/DNumericVersion=$Numeric" "/DArch=$Arch" "/DStage=$Stage" "/DOutput=$Out" "$Here\sikemux.iss"
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed" }

Get-ChildItem $Stage, $Out | Where-Object { -not $_.PSIsContainer } |
  ForEach-Object { "{0,-40} {1,8:N1} MB" -f $_.Name, ($_.Length / 1MB) }
