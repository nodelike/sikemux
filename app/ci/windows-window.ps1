# Opens the app in a real window on a Windows runner and keeps a picture of it.
# PrintWindow draws the window itself, so a first-run screen over the desktop does not hide it.
param(
    [Parameter(Mandatory)] [string] $App,
    [Parameter(Mandatory)] [string] $Out,
    [int] $Settle = 20
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$env:LOCALAPPDATA = (New-Item -ItemType Directory -Force -Path (Join-Path $Out 'local')).FullName

Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System; using System.Drawing; using System.Runtime.InteropServices;
public static class Grab {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  public static bool Save(IntPtr h, string path) {
    RECT r; GetWindowRect(h, out r);
    using (var bmp = new Bitmap(r.R - r.L, r.B - r.T)) {
      bool ok;
      using (var g = Graphics.FromImage(bmp)) { var dc = g.GetHdc(); ok = PrintWindow(h, dc, 2); g.ReleaseHdc(dc); }
      bmp.Save(path); return ok;
    }
  }
}
"@

$proc = Start-Process -FilePath $App -PassThru `
    -RedirectStandardError (Join-Path $Out 'stderr.txt') -RedirectStandardOutput (Join-Path $Out 'stdout.txt')
$deadline = (Get-Date).AddSeconds(60)
while ((Get-Date) -lt $deadline -and -not $proc.HasExited) {
    $proc.Refresh()
    if ($proc.MainWindowHandle -ne [IntPtr]::Zero) { break }
    Start-Sleep -Milliseconds 250
}
if ($proc.HasExited) {
    Get-Content (Join-Path $Out 'stderr.txt')
    throw "the app exited with $($proc.ExitCode) before its window opened"
}
if ($proc.MainWindowHandle -eq [IntPtr]::Zero) { throw 'no window opened within a minute' }
Start-Sleep -Seconds $Settle
$proc.Refresh()
if (-not [Grab]::Save($proc.MainWindowHandle, (Join-Path $Out 'window.png'))) { throw 'PrintWindow drew nothing' }
"window: $($proc.MainWindowTitle)" | Out-File (Join-Path $Out 'state.txt')
Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion | Out-File -Append (Join-Path $Out 'state.txt')
Stop-Process -Id $proc.Id
Get-Content (Join-Path $Out 'stderr.txt')
