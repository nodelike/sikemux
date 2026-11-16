# Opens the app on a Windows runner with the core it starts itself, opens a project through the
# CLI, a terminal by its key, then a chat with the film's stand-in Claude that answers, reads a
# file and opens a browser tab, and keeps a picture of each stage.
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
$claude = (Resolve-Path (Join-Path (Split-Path $App) 'sikemux-fake-claude.exe')).Path
# A debug core lets a program act as the person only from where it knows it, examples\remote.
$examples = Join-Path (Split-Path $App) 'examples'
Copy-Item (Join-Path $examples 'fake_chat.exe') (Join-Path $examples 'remote.exe') -Force
$chat = (Resolve-Path (Join-Path $examples 'remote.exe')).Path
$replies = Join-Path $Out 'replies.json'
Set-Content -Path $replies -Value (ConvertTo-Json -Depth 8 -InputObject @(
    @{
        when = 'look'
        steps = @(
            @{ say = 'Reading the README first.' },
            @{ tool = 'Read'; input = @{ file_path = (Join-Path $project 'README.md') }; result = '# A project opened by the window run'; ms = 5000 },
            @{ tool = 'mcp__sikemux-tools__browser_navigate'; call = 'browser.navigate'; input = @{ url = 'https://example.com' }; ms = 1200 },
            @{ say = 'It is a project opened by the window run, and example.com is open beside this chat.' }
        )
    }
))
$env:FAKE_CLAUDE_SCRIPT = $replies

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
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr h);
  // How many colours a picture of the window holds in an area given in the app's own points.
  public static int Colours(IntPtr h, string path, float x, float y, float w, float h2) {
    RECT r; GetWindowRect(h, out r);
    var client = new POINT(); ClientToScreen(h, ref client);
    float scale = GetDpiForWindow(h) / 96f;
    var seen = new System.Collections.Generic.HashSet<int>();
    using (var bmp = new Bitmap(path)) {
      int left = client.X - r.L + (int)(x * scale), top = client.Y - r.T + (int)(y * scale);
      int right = Math.Min(bmp.Width, left + (int)(w * scale)), bottom = Math.Min(bmp.Height, top + (int)(h2 * scale));
      for (int py = Math.Max(0, top); py < bottom; py += 2)
        for (int px = Math.Max(0, left); px < right; px += 2) seen.Add(bmp.GetPixel(px, py).ToArgb());
    }
    return seen.Count;
  }
  public static void Screen(IntPtr h, string path) {
    RECT r; GetWindowRect(h, out r);
    using (var bmp = new Bitmap(r.R - r.L, r.B - r.T)) {
      using (var g = Graphics.FromImage(bmp)) { g.CopyFromScreen(r.L, r.T, 0, 0, bmp.Size); }
      bmp.Save(path);
    }
  }
}
"@

# The app is built with its drive feature, which takes key steps on this pipe.
$env:SIKEMUX_GPUI_DRIVE = '\\.\pipe\sikemux-window-run'
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

function Drive([string] $Step, [string] $Steps) {
    $pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', 'sikemux-window-run', [System.IO.Pipes.PipeDirection]::InOut)
    $pipe.Connect(10000)
    $writer = New-Object System.IO.StreamWriter($pipe)
    $writer.Write("$Steps`nend`n"); $writer.Flush()
    $answer = (New-Object System.IO.StreamReader($pipe)).ReadToEnd()
    $pipe.Dispose()
    "$Step`n$answer" | Out-File -Append (Join-Path $Out 'steps.txt')
    Write-Host "${Step}: $answer"
    $answer
}

Start-Sleep -Seconds $Settle
Picture 'connected'

Ask-Core -Step 'open the project' -Words 'open', $project | Out-Null
Start-Sleep -Seconds 10
Drive 'open a terminal' "activate`nkey ctrl+t`nwait 3" | Out-Null
Start-Sleep -Seconds 5
Picture 'terminal'
$folder = Join-Path $Out 'terminal-folder.txt'
Drive 'the terminal says its folder' "text `$pwd.Path | Set-Content $folder`nkey enter`nwait 3" | Out-Null

$env:FAKE_PROVIDER = 'claude'
$env:FAKE_PROJECT = $project
# It leaves at once: while connected it stands in front of the app and would be handed the
# browser call, which the Read's five seconds hold back until it has gone.
$env:FAKE_PAUSE_MS = '0'
$socket = Join-Path $env:LOCALAPPDATA 'Sikemux\core.dev.sock'
$ErrorActionPreference = 'Continue'
$answer = (& $chat $socket $claude 'look at this project' 2>&1 | Out-String)
$ErrorActionPreference = 'Stop'
"start a chat on the stand-in (exit $LASTEXITCODE)`n$answer" | Out-File -Append (Join-Path $Out 'steps.txt')
Write-Host "start a chat on the stand-in (exit $LASTEXITCODE): $answer"
if ($LASTEXITCODE -ne 0) { $failed += 'start a chat on the stand-in' }
Start-Sleep -Seconds 2
Picture 'reading'
Start-Sleep -Seconds 28
$state = Drive 'what the window holds' 'state'
if (-not $state.Contains('example.com')) { $failed += "the stand-in's browser tab" }
Picture 'window'
# example.com is grey with dark text, so a page drawn in its pane is more than a few colours.
$bounds = ([regex]::Match($state, '"pageBounds":\[([^\]]+)\]').Groups[1].Value -split ',') | ForEach-Object { [float] $_ }
if ($bounds.Count -ne 4) { $failed += 'the desk places its page' }
else {
    $colours = [Grab]::Colours($proc.MainWindowHandle, (Join-Path $Out 'window.png'), $bounds[0], $bounds[1], $bounds[2], $bounds[3])
    "page area: $colours colours" | Out-File -Append (Join-Path $Out 'steps.txt')
    if ($colours -lt 8) { $failed += "the page drawn in its pane ($colours colours)" }
}
# PrintWindow can miss a page the window composes from another process; the screen shows it.
[Grab]::Screen($proc.MainWindowHandle, (Join-Path $Out 'window-screen.png'))
"window: $($proc.MainWindowTitle)" | Out-File (Join-Path $Out 'state.txt')

Stop-Process -Id $proc.Id
Get-Process sikemux-host, sikemux-fake-claude -ErrorAction SilentlyContinue | Stop-Process
Get-Content (Join-Path $Out 'stdout.txt'), (Join-Path $Out 'stderr.txt')

# The prompt shows the shell's folder, and Windows' \\?\ prefix must reach neither it nor the
# project's path the core stores.
$verbatim = '\\?\'
if (-not (Test-Path $folder)) { $failed += 'the terminal says its folder' }
elseif ((Get-Content $folder -Raw).Contains($verbatim)) { $failed += "the terminal's folder is $(Get-Content $folder -Raw)" }
$stored = Get-ChildItem -Recurse -File (Join-Path $env:LOCALAPPDATA 'Sikemux'), $data.FullName -ErrorAction SilentlyContinue |
    Where-Object { $_.Extension -notin '.log', '.exe', '.dll' -and "$(Get-Content -Raw $_.FullName -ErrorAction SilentlyContinue)".Contains($verbatim) }
if ($stored) { $failed += "the core stored \\?\ paths in $($stored.Name -join ', ')" }
if ($failed.Count -gt 0) { throw "these steps failed: $($failed -join ', ')" }
