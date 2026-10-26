$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms

$source = Split-Path -Parent $MyInvocation.MyCommand.Path
$version = (Get-Content (Join-Path $source 'version.txt') -Raw).Trim()
$destination = Join-Path $env:LOCALAPPDATA 'Programs\Sikemux'

$running = Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($destination, [System.StringComparison]::OrdinalIgnoreCase) }
if ($running) {
    [System.Windows.Forms.MessageBox]::Show('Sikemux is running. Quit it, stop its core with "sikemux-host stop", then run setup again.', 'Sikemux setup') | Out-Null
    exit 1
}

New-Item -ItemType Directory -Force $destination | Out-Null
foreach ($file in 'sikemux.exe', 'sikemux-host.exe', 'sikemux_pi_tools.ts', 'uninstall.ps1') {
    Copy-Item (Join-Path $source $file) $destination -Force
}

$programs = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $programs 'Sikemux.lnk'))
$shortcut.TargetPath = Join-Path $destination 'sikemux.exe'
$shortcut.WorkingDirectory = $destination
$shortcut.Save()

$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Sikemux'
New-Item $key -Force | Out-Null
Set-ItemProperty $key DisplayName 'Sikemux'
Set-ItemProperty $key DisplayVersion $version
Set-ItemProperty $key Publisher 'Sikemux'
Set-ItemProperty $key InstallLocation $destination
Set-ItemProperty $key DisplayIcon (Join-Path $destination 'sikemux.exe')
Set-ItemProperty $key UninstallString "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$(Join-Path $destination 'uninstall.ps1')`""
Set-ItemProperty $key NoModify 1 -Type DWord
Set-ItemProperty $key NoRepair 1 -Type DWord

Start-Process (Join-Path $destination 'sikemux.exe')
