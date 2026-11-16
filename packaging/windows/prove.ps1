# Proves an installer on a clean runner, which runs as an administrator: a silent install, the app opening from where it was
# installed, a silent reinstall over the running app that starts it again, and an uninstall
# that leaves only the person's data.
param(
  [Parameter(Mandatory)][string]$Installer,
  [Parameter(Mandatory)][string]$Version,
  [Parameter(Mandatory)][string]$Out
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force $Out | Out-Null

$Dir = "$env:ProgramFiles\Sikemux"
$App = "$Dir\sikemux.exe"
$Shortcut = "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\Sikemux.lnk"
$Uninstall = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{6D6F1A52-3B1C-4B7E-9C55-5A3E2D8B7C41}_is1"
$Preferences = "$env:LOCALAPPDATA\sikemux\preferences.json"
$CliCopy = "$env:APPDATA\sikemux\bin"
$Core = "$Dir\sikemux-host.exe"
$SignIn = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$SignInName = "com.nodelike.sikemux.core"
$SignInFile = "$env:LOCALAPPDATA\sikemux\com.nodelike.sikemux.core.json"
$Service = "Sikemux"
$WatchFile = "$env:LOCALAPPDATA\sikemux\com.nodelike.sikemux.core.watch"

function Check($Holds, $What) {
  if (-not $Holds) { throw "failed: $What" }
  Write-Host "ok: $What"
}

function Run-Setup($Arguments, $Log) {
  $Setup = Start-Process $Installer -ArgumentList ($Arguments + "/LOG=`"$Out\$Log`"") -PassThru
  # Not -Wait: it also waits for the app setup relaunches, which runs on.
  if (-not $Setup.WaitForExit(300000)) { throw "failed: $Log still runs after 5 min" }
  Check ($Setup.ExitCode -eq 0) "$Log exits 0 (it exited $($Setup.ExitCode))"
}

function Service-Running {
  $Found = Get-Service $Service -ErrorAction SilentlyContinue
  $Found -and $Found.Status -eq "Running" -and $Found.StartType -eq "Automatic"
}

function Installed-Running {
  @(Get-Process sikemux -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $App })
}

# Through a pipe on purpose: a core that kept the caller's output open would hang this.
function Core-Run($Command) {
  & $Core $Command | Out-Host
  $LASTEXITCODE
}

function Core-Answers {
  & $Core status | Out-Null
  $LASTEXITCODE -eq 0
}

function Wait-Core($Seconds) {
  $Deadline = (Get-Date).AddSeconds($Seconds)
  while ((Get-Date) -lt $Deadline) {
    if (Core-Answers) { return $true }
    Start-Sleep -Milliseconds 500
  }
  $false
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
$HasCore = Test-Path $Core
if ($HasCore) {
  $Info = (Get-Item $Core).VersionInfo
  Check ($Info.ProductName -eq "Sikemux" -and $Info.ProductVersion -eq $Version) "sikemux-host.exe says Sikemux $Version"
  Check (Service-Running) "the $Service service runs and starts at boot"
}
Check ((Get-Content $Preferences -Raw) -match '"shareUsageData":false') "unticking sharing turns usage data and crash reports off"

Start-Process $App
$Window = Wait-Window 60
Check $Window "the installed app opens a window"
Write-Host "window: $($Window.MainWindowTitle)"

if ($HasCore) {
  Check (Wait-Core 60) "the app starts its core"
  Check ((Core-Run enable) -eq 0) "sikemux-host enable exits 0"
  $Command = (Get-ItemProperty $SignIn -Name $SignInName -ErrorAction SilentlyContinue).$SignInName
  Check ($Command -like "`"$App`" --start-core *") "the core is set to start at sign-in from the app: $Command"
  Installed-Running | Stop-Process -Force
  Check ((Core-Run stop) -eq 0) "sikemux-host stop exits 0"
  Check (-not (Core-Answers)) "the core and its watcher are stopped"
  # Exactly what Windows runs at sign-in.
  Start-Process cmd.exe -ArgumentList "/d /s /c `"$Command`"" -WindowStyle Hidden
  Check (Wait-Core 30) "the sign-in command starts a core that answers"
  Start-Process $App
  $Window = Wait-Window 60
  Check $Window "the app opens again beside the core"
}

$Before = $Window.Id
Run-Setup ($Silent + "/relaunch=1") "update.log"
$Window = Wait-Window 60
Check ($Window -and $Window.Id -ne $Before) "a silent reinstall over the running app starts it again"
if ($HasCore) {
  Check (Wait-Core 60) "the core answers after the reinstall"
  Check (Service-Running) "the $Service service runs again after the reinstall"
}
& "$Dir\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART "/LOG=$Out\uninstall.log" | Out-Null
# The uninstaller hands its work to a copy of itself and returns at once.
$Deadline = (Get-Date).AddSeconds(60)
while ((Test-Path $Dir) -and (Get-Date) -lt $Deadline) { Start-Sleep -Milliseconds 500 }
Check (-not (Test-Path $Dir)) "the install folder is gone"
Check (-not (Test-Path $Shortcut)) "the Start menu entry is gone"
Check (-not (Test-Path $Uninstall)) "Apps & features no longer lists it"
Check (-not (Test-Path $CliCopy)) "the agents' sikemux command is gone"
Check (-not (Get-ItemProperty $SignIn -Name $SignInName -ErrorAction SilentlyContinue)) "the core no longer starts at sign-in"
Check (-not (Test-Path $SignInFile)) "the sign-in file is gone"
Check (-not (Test-Path $WatchFile)) "the core's watch file is gone"
Check (@(Get-Process sikemux-host -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Core }).Count -eq 0) "the core is stopped"
Check (-not (Get-Service $Service -ErrorAction SilentlyContinue)) "the $Service service is removed"
Check ((Installed-Running).Count -eq 0) "nothing runs from the install folder"
Check (Test-Path $Preferences) "the person's data stays"
# Every failed check throws, so reaching here passes, whatever the last program run returned.
exit 0
