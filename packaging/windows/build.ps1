# Packs release builds of sikemux.exe (and sikemux-host.exe, once it is built) into the
# per-user installer, after naming the product and its version in every exe.
param(
  [Parameter(Mandatory)][ValidateSet("x64", "arm64")][string]$Arch,
  [Parameter(Mandatory)][string]$Version,
  [Parameter(Mandatory)][string]$Binaries,
  [Parameter(Mandatory)][string]$Out
)
$ErrorActionPreference = "Stop"
$Here = $PSScriptRoot
$Root = Resolve-Path "$Here\..\.."

$RceditUrl = "https://github.com/electron/rcedit/releases/download/v2.0.0/rcedit-x64.exe"
$RceditSha256 = "3e7801db1a5edbec91b49a24a094aad776cb4515488ea5a4ca2289c400eade2a"

if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)(-nightly\.(\d+))?$') {
  throw "$Version is neither a stable nor a nightly version"
}
$Build = if ($Matches[5]) { $Matches[5] } else { "0" }
$Numeric = "$($Matches[1]).$($Matches[2]).$($Matches[3]).$Build"

$Stage = Join-Path $env:RUNNER_TEMP "sikemux-stage-$Arch"
$Tools = Join-Path $env:RUNNER_TEMP "sikemux-tools"
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $Stage, $Tools, $Out | Out-Null

# An .ico holds PNG frames as they are, so the Linux icons go in unchanged.
$Frames = 32, 128, 256 | ForEach-Object { [IO.File]::ReadAllBytes("$Root\brand\linux\icon-$_.png") }
$Sizes = 32, 128, 256
$Icon = New-Object IO.MemoryStream
$Writer = New-Object IO.BinaryWriter $Icon
$Writer.Write([uint16]0); $Writer.Write([uint16]1); $Writer.Write([uint16]$Frames.Count)
$Offset = 6 + 16 * $Frames.Count
for ($i = 0; $i -lt $Frames.Count; $i++) {
  $Side = if ($Sizes[$i] -ge 256) { 0 } else { $Sizes[$i] }
  $Writer.Write([byte]$Side); $Writer.Write([byte]$Side); $Writer.Write([byte]0); $Writer.Write([byte]0)
  $Writer.Write([uint16]1); $Writer.Write([uint16]32)
  $Writer.Write([uint32]$Frames[$i].Length); $Writer.Write([uint32]$Offset)
  $Offset += $Frames[$i].Length
}
$Frames | ForEach-Object { $Writer.Write($_) }
[IO.File]::WriteAllBytes("$Stage\sikemux.ico", $Icon.ToArray())

$Rcedit = "$Tools\rcedit-x64.exe"
if (-not (Test-Path $Rcedit)) { Invoke-WebRequest $RceditUrl -OutFile $Rcedit }
if ((Get-FileHash $Rcedit -Algorithm SHA256).Hash -ne $RceditSha256) { throw "rcedit does not match its pinned hash" }

$Programs = @{ "sikemux.exe" = "Sikemux"; "sikemux-host.exe" = "Sikemux Core" }
foreach ($Name in $Programs.Keys) {
  $Built = Join-Path $Binaries $Name
  if (-not (Test-Path $Built)) {
    if ($Name -eq "sikemux.exe") { throw "$Built was not built" }
    Write-Host "$Name was not built; the installer carries the app alone"
    continue
  }
  Copy-Item $Built $Stage
  & $Rcedit "$Stage\$Name" `
    --set-icon "$Stage\sikemux.ico" `
    --set-file-version $Numeric `
    --set-product-version $Version `
    --set-version-string ProductName "Sikemux" `
    --set-version-string FileDescription $Programs[$Name] `
    --set-version-string CompanyName "Sikemux" `
    --set-version-string LegalCopyright "Copyright (c) Sikemux contributors" `
    --set-version-string OriginalFilename $Name
  if ($LASTEXITCODE -ne 0) { throw "rcedit failed on $Name" }
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
