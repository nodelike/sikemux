# Opens the app on a Windows runner with the core it starts itself, then opens a project, a
# terminal and a browser tab through the core's own tools, and keeps a picture of each stage.
param(
    [Parameter(Mandatory)] [string] $App,
    [Parameter(Mandatory)] [string] $Out,
    [int] $Settle = 45
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$env:LOCALAPPDATA = (New-Item -ItemType Directory -Force -Path (Join-Path $Out 'local')).FullName
$env:XDG_DATA_HOME = (New-Item -ItemType Directory -Force -Path (Join-Path $Out 'data')).FullName
$project = (New-Item -ItemType Directory -Force -Path (Join-Path $Out 'project')).FullName
Set-Content -Path (Join-Path $project 'README.md') -Value '# A project opened by the window run'
# Past the first-run welcome, which would cover the workspace.
$data = New-Item -ItemType Directory -Force -Path (Join-Path $env:XDG_DATA_HOME 'com.nodelike.sikemux.dev')
Set-Content -Path (Join-Path $data 'preferences.json') -Value '{"onboardingComplete":true}'
$cli = Join-Path (Split-Path $App) 'sikemux-host.exe'
if (-not (Test-Path $cli)) { throw "$cli is not built beside the app" }
$cli = (Resolve-Path $cli).Path

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

function Picture([string] $name) {
    $proc.Refresh()
    if ($proc.HasExited) {
        Get-Content (Join-Path $Out 'stderr.txt')
        throw "the app exited with $($proc.ExitCode) before $name"
    }
    if (-not [Grab]::Save($proc.MainWindowHandle, (Join-Path $Out "$name.png"))) { throw "PrintWindow drew nothing for $name" }
}

$failed = @()
# Windows PowerShell drops the double quotes inside an argument to a program, so JSON goes with
# each one escaped.
function Ask-Core {
    param([string] $Step, [string[]] $Words)
    # A program's own error lines are part of its answer, not a failure of the script.
    $ErrorActionPreference = 'Continue'
    $quoted = $Words | ForEach-Object { $_.Replace('"', '\"') }
    Push-Location $project
    try {
        $answer = (& $cli @quoted 2>&1 | Out-String)
        $code = $LASTEXITCODE
    } finally { Pop-Location }
    "$Step (exit $code)`n$answer" | Out-File -Append (Join-Path $Out 'steps.txt')
    Write-Host "$Step (exit $code): $answer"
    if ($code -ne 0) { $script:failed += $Step }
    $answer
}

Start-Sleep -Seconds $Settle
Picture 'connected'

Ask-Core -Step 'open the project' -Words 'open', $project | Out-Null
Start-Sleep -Seconds 10
$started = Ask-Core -Step 'start a terminal' -Words 'tool', 'process.start', '{"command":"ping -n 900 127.0.0.1","label":"ping","idempotencyKey":"window-run"}'
$execution = ''
try { $execution = ($started | ConvertFrom-Json).executionId } catch {}
if ($execution) {
    Ask-Core -Step 'show the terminal' -Words 'tool', 'ui.open', (@{ kind = 'terminal'; executionId = $execution } | ConvertTo-Json -Compress) | Out-Null
} else { $failed += 'no executionId' }
$env:SIKEMUX_AGENT_ID = 'window-run'
Ask-Core -Step 'open a browser tab' -Words 'tool', 'browser.navigate', '{"url":"https://example.com"}' | Out-Null
Remove-Item Env:SIKEMUX_AGENT_ID
Start-Sleep -Seconds 15
Picture 'window'
"window: $($proc.MainWindowTitle)" | Out-File (Join-Path $Out 'state.txt')

Stop-Process -Id $proc.Id
Get-Process sikemux-host -ErrorAction SilentlyContinue | Stop-Process
Get-Content (Join-Path $Out 'stdout.txt'), (Join-Path $Out 'stderr.txt')
if ($failed.Count -gt 0) { throw "these steps failed: $($failed -join ', ')" }
