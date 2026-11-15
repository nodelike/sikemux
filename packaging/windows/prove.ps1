# Proves an installer on a clean runner: a silent install, the app opening from where it was
# installed, a silent reinstall over the running app that starts it again, and an uninstall
# that leaves only the person's data.
param(
  [Parameter(Mandatory)][string]$Installer,
  [Parameter(Mandatory)][string]$Version,
  [Parameter(Mandatory)][string]$Out
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force $Out | Out-Null

$Dir = "$env:LOCALAPPDATA\Programs\Sikemux"
$App = "$Dir\sikemux.exe"
$Shortcut = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Sikemux.lnk"
$Uninstall = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{6D6F1A52-3B1C-4B7E-9C55-5A3E2D8B7C41}_is1"
$Preferences = "$env:LOCALAPPDATA\sikemux\preferences.json"
$CliCopy = "$env:APPDATA\sikemux\bin"
$SignIn = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"

function Check($Holds, $What) {
  if (-not $Holds) { throw "failed: $What" }
  Write-Host "ok: $What"
}

function Run-Setup($Arguments, $Log) {
  $Setup = Start-Process $Installer -ArgumentList ($Arguments + "/LOG=`"$Out\$Log`"") -Wait -PassThru
  Check ($Setup.ExitCode -eq 0) "$Log exits 0 (it exited $($Setup.ExitCode))"
}

function Installed-Running {
  @(Get-Process sikemux -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $App })
}

function Wait-Window($Seconds) {
  $Deadline = (Get-Date).AddSeconds($Seconds)
  while ((Get-Date) -lt $Deadline) {
    $Window = Installed-Running | Where-Object { $_.MainWindowHandle -ne [IntPtr]::Zero } | Select-Object -First 1
    if ($Window) { return $Window }
    Start-Sleep -Milliseconds 500
  }
}

$Silent = "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"

Run-Setup ($Silent + '/MERGETASKS="!shareusage"') "install.log"
Check (Test-Path $App) "the app is in $Dir"
Check (Test-Path $Shortcut) "the Start menu has Sikemux"
$Entry = Get-ItemProperty $Uninstall
Check ($Entry.DisplayVersion -eq $Version) "Apps & features lists $Version"
$Info = (Get-Item $App).VersionInfo
Check ($Info.ProductName -eq "Sikemux" -and $Info.ProductVersion -eq $Version) "sikemux.exe says Sikemux $Version"
if (Test-Path "$Dir\sikemux-host.exe") {
  $Core = (Get-Item "$Dir\sikemux-host.exe").VersionInfo
  Check ($Core.ProductName -eq "Sikemux" -and $Core.ProductVersion -eq $Version) "sikemux-host.exe says Sikemux $Version"
}
Check ((Get-Content $Preferences -Raw) -match '"shareUsageData":false') "unticking sharing turns usage data and crash reports off"

Start-Process $App
$Window = Wait-Window 60
Check $Window "the installed app opens a window"
Write-Host "window: $($Window.MainWindowTitle)"

$Before = $Window.Id
Run-Setup ($Silent + "/relaunch=1") "update.log"
$Window = Wait-Window 60
Check ($Window -and $Window.Id -ne $Before) "a silent reinstall over the running app starts it again"

# What the core writes when it is set to start at sign-in.
Set-ItemProperty $SignIn "com.nodelike.sikemux.core" "`"$App`" --start-core"
& "$Dir\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART "/LOG=$Out\uninstall.log" | Out-Null
# The uninstaller hands its work to a copy of itself and returns at once.
$Deadline = (Get-Date).AddSeconds(60)
while ((Test-Path $Dir) -and (Get-Date) -lt $Deadline) { Start-Sleep -Milliseconds 500 }
Check (-not (Test-Path $Dir)) "the install folder is gone"
Check (-not (Test-Path $Shortcut)) "the Start menu entry is gone"
Check (-not (Test-Path $Uninstall)) "Apps & features no longer lists it"
Check (-not (Test-Path $CliCopy)) "the agents' sikemux command is gone"
Check (-not (Get-ItemProperty $SignIn -Name "com.nodelike.sikemux.core" -ErrorAction SilentlyContinue)) "the core no longer starts at sign-in"
Check ((Installed-Running).Count -eq 0) "nothing runs from the install folder"
Check (Test-Path $Preferences) "the person's data stays"
