$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms

$destination = Split-Path -Parent $MyInvocation.MyCommand.Path

$running = Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($destination, [System.StringComparison]::OrdinalIgnoreCase) }
if ($running) {
    [System.Windows.Forms.MessageBox]::Show('Sikemux is running. Quit it, stop its core with "sikemux-host stop", then uninstall again.', 'Sikemux uninstall') | Out-Null
    exit 1
}

Remove-Item (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Sikemux.lnk') -Force -ErrorAction SilentlyContinue
Remove-Item 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Sikemux' -Recurse -Force -ErrorAction SilentlyContinue
Start-Process cmd.exe -WindowStyle Hidden -ArgumentList '/c', "ping -n 3 127.0.0.1 >nul & rmdir /s /q `"$destination`""
