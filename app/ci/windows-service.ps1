# Installs the Sikemux service from a folder only administrators write, has it start this person's
# core, and checks what Windows and the core report about that core. Needs an elevated runner.
param(
    [Parameter(Mandatory)] [string] $Built,
    [Parameter(Mandatory)] [string] $Sign,
    [Parameter(Mandatory)] [string] $Probe,
    [Parameter(Mandatory)] [string] $Remote,
    [Parameter(Mandatory)] [string] $Out
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$install = Join-Path $env:ProgramFiles 'Sikemux proof'
New-Item -ItemType Directory -Force -Path $install | Out-Null
Copy-Item $Built $install
$cli = Join-Path $install (Split-Path -Leaf $Built)

# The names the service passes the core, and the PATH it builds.
$environment = @(
    'SystemRoot', 'SystemDrive', 'windir', 'ComSpec', 'PATHEXT', 'OS', 'PROCESSOR_ARCHITECTURE',
    'NUMBER_OF_PROCESSORS', 'COMPUTERNAME', 'ProgramData', 'ProgramFiles', 'ProgramFiles(x86)',
    'ProgramW6432', 'CommonProgramFiles', 'CommonProgramFiles(x86)', 'CommonProgramW6432', 'PUBLIC',
    'ALLUSERSPROFILE', 'USERNAME', 'USERDOMAIN', 'USERPROFILE', 'HOMEDRIVE', 'HOMEPATH', 'APPDATA',
    'LOCALAPPDATA', 'TEMP', 'TMP', 'PATH'
)

$failed = @()
function Check([string] $What, [bool] $Held, [string] $Detail = '') {
    $line = "$(if ($Held) { 'ok  ' } else { 'FAIL' }) $What$(if ($Detail) { ": $Detail" })"
    $line | Out-File -Encoding utf8 -Append (Join-Path $Out 'checks.txt')
    Write-Host $line
    if (-not $Held) { $script:failed += $What }
}

function Host-Command([string[]] $Words) {
    $ErrorActionPreference = 'Continue'
    $answer = (& $cli @Words 2>&1 | Out-String)
    "$($Words -join ' ') (exit $LASTEXITCODE)`n$answer" | Out-File -Encoding utf8 -Append (Join-Path $Out 'steps.txt')
    [pscustomobject]@{ Code = $LASTEXITCODE; Text = $answer }
}

function Status {
    $ran = Host-Command 'status', '--json'
    try { $ran.Text | ConvertFrom-Json } catch { $null }
}

function Under([string] $Path, [string[]] $Folders) {
    $full = [System.IO.Path]::GetFullPath($Path)
    foreach ($folder in $Folders) {
        $prefix = [System.IO.Path]::GetFullPath($folder).TrimEnd('\') + '\'
        if ($full.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { return $true }
    }
    $false
}

function Check-Core([string] $When) {
    $status = Status
    Check "$When, the core runs" ($null -ne $status -and $status.running) "pid $($status.pid)"
    $trust = $status.trust
    Check "$When, the core runs at High" ($trust.integrity -eq 'high') "$($trust.integrity)"
    $outside = @($trust.libraries | Where-Object { -not (Under $_ @($install, $env:windir)) })
    Check "$When, every library is under the install folder or Windows" (
        $trust.libraries.Count -gt 0 -and $outside.Count -eq 0
    ) "$($trust.libraries.Count) loaded; outside: $($outside -join ', ')"
    $extra = @($trust.environment | Where-Object { $environment -notcontains $_ })
    Check "$When, the environment holds only the fixed names" (
        $trust.environment.Count -gt 0 -and $extra.Count -eq 0
    ) "$($trust.environment -join ' '); not on the list: $($extra -join ', ')"
    $status
}

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class Dialogs {
    delegate bool Each(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] static extern bool EnumWindows(Each each, IntPtr data);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr window, StringBuilder text, int size);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr window, StringBuilder text, int size);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern uint GetDlgItemText(IntPtr window, int item, StringBuilder text, int size);
    [DllImport("user32.dll")] static extern IntPtr GetDlgItem(IntPtr window, int item);
    [DllImport("user32.dll")] static extern IntPtr SendMessage(IntPtr window, uint message, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr window, uint message, IntPtr w, IntPtr l);

    public static IntPtr Find(uint pid, string title) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((window, data) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == pid && IsWindowVisible(window) && Text(window) == title) { found = window; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    static string Text(IntPtr window) {
        var text = new StringBuilder(256);
        GetWindowText(window, text, text.Capacity);
        return text.ToString();
    }

    public static string Class(IntPtr window) {
        var name = new StringBuilder(256);
        GetClassName(window, name, name.Capacity);
        return name.ToString();
    }

    public static string Message(IntPtr window) {
        var text = new StringBuilder(1024);
        GetDlgItemText(window, 0xFFFF, text, text.Capacity);
        return text.ToString();
    }

    public static bool HasButton(IntPtr window, int button) { return GetDlgItem(window, button) != IntPtr.Zero; }

    public static int DefaultButton(IntPtr window) { return (int)SendMessage(window, 0x0400, IntPtr.Zero, IntPtr.Zero) & 0xFFFF; }

    public static void Press(IntPtr window, int button) { PostMessage(window, 0x0111, (IntPtr)button, IntPtr.Zero); }
}
'@

$yes = 6
$no = 7

function Start-Sign([string] $Socket, [string] $Name) {
    $report = Join-Path $Out "sign-$Name.txt"
    Remove-Item $report -ErrorAction SilentlyContinue
    & $Sign $Socket $report
    $report
}

function Wait-For([scriptblock] $Ready, [int] $Seconds) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while (-not ($found = & $Ready) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 250 }
    $found
}

function Answer-Of([string] $Report) {
    Wait-For { if (Test-Path $Report) { Get-Content -Raw $Report } } 30
}

function Check-Window($Core) {
    $corePid = [uint32]$Core.pid
    $sessions = "the core is in session $((Get-Process -Id $corePid).SessionId), this step in $((Get-Process -Id $PID).SessionId)"
    $first = Start-Sign $Core.socket 'no'
    $window = Wait-For { $found = [Dialogs]::Find($corePid, 'Sikemux'); if ($found -ne [IntPtr]::Zero) { $found } } 30
    Check 'a signing request from a Medium program opens a window of the core''s' ($null -ne $window) $sessions
    if ($null -eq $window) {
        Check 'the request is answered without the window' (Test-Path $first) "$(Get-Content -Raw $first -ErrorAction SilentlyContinue)"
        return
    }
    Check 'the window is a Yes/No box with No as the default' (
        [Dialogs]::Class($window) -eq '#32770' -and [Dialogs]::HasButton($window, $yes) -and
        [Dialogs]::HasButton($window, $no) -and [Dialogs]::DefaultButton($window) -eq $no
    ) "class $([Dialogs]::Class($window)), default $([Dialogs]::DefaultButton($window)): $([Dialogs]::Message($window))"
    Start-Sleep -Seconds 3
    Check 'the request waits while the window is open' (-not (Test-Path $first))

    $second = Answer-Of (Start-Sign $Core.socket 'second')
    Check 'a second request is refused while the window is open' (
        $second -match 'another request is waiting for your answer'
    ) "$second".Trim()

    [Dialogs]::Press($window, $no)
    $answer = Answer-Of $first
    Check 'the signing program runs at Medium' ($answer -match '(?m)^integrity medium$') "$answer".Trim()
    Check 'answered No, the request is refused' ($answer -match 'you said no') "$answer".Trim()

    $third = Start-Sign $Core.socket 'yes'
    $window = Wait-For { $found = [Dialogs]::Find($corePid, 'Sikemux'); if ($found -ne [IntPtr]::Zero) { $found } } 30
    if ($null -ne $window) { [Dialogs]::Press($window, $yes) }
    $answer = Answer-Of $third
    Check 'answered Yes, the request is signed' ($null -ne $window -and $answer -match '(?m)^signed \S+') "$answer".Trim()
}

# Stands in for the accounts server a dev core reaches at 127.0.0.1:4000, approving the first
# code it hands out.
$accounts = {
    $listener = [System.Net.HttpListener]::new()
    $listener.Prefixes.Add('http://127.0.0.1:4000/')
    $listener.Start()
    while ($listener.IsListening) {
        $context = $listener.GetContext()
        $now = [DateTime]::UtcNow
        $answer = switch ($context.Request.Url.AbsolutePath) {
            '/v1/host-sign-ins' { @{
                code = 'c' * 64; userCode = 'BCDF GHJK'; verificationUrl = 'http://127.0.0.1:4000/approve'
                expiresAt = $now.AddMinutes(10).ToString('yyyy-MM-ddTHH:mm:ssZ'); interval = 1
            } }
            '/v1/host-sign-ins/poll' { @{ status = 'approved'; account = 'user_login'; nonce = 'b' * 64 } }
            '/v1/host-sign-ins/finish' { @{
                key = 'k'; role = 'host'; name = 'proof'; platform = 'windows'
                createdAt = $now.ToString('yyyy-MM-ddTHH:mm:ssZ'); lastSeenAt = $null
            } }
        }
        $context.Response.StatusCode = if ($answer) { 200 } else { 404 }
        $body = [Text.Encoding]::UTF8.GetBytes($(if ($answer) { $answer | ConvertTo-Json } else { '{}' }))
        $context.Response.ContentType = 'application/json'
        $context.Response.OutputStream.Write($body, 0, $body.Length)
        $context.Response.Close()
    }
}

function Check-Login($Core) {
    $corePid = [uint32]$Core.pid
    $server = Start-Job $accounts
    Wait-For { try { [Net.Sockets.TcpClient]::new('127.0.0.1', 4000).Close(); $true } catch { $false } } 30 | Out-Null
    $said = Join-Path $Out 'login.txt'
    $login = Start-Process $cli -ArgumentList 'login', '--json' -NoNewWindow -PassThru `
        -RedirectStandardOutput $said -RedirectStandardError (Join-Path $Out 'login-errors.txt')
    $null = $login.Handle
    $asked = @()
    $deadline = (Get-Date).AddSeconds(90)
    while (-not $login.HasExited -and (Get-Date) -lt $deadline) {
        $window = [Dialogs]::Find($corePid, 'Sikemux')
        if ($window -ne [IntPtr]::Zero) {
            $asked += [Dialogs]::Message($window)
            [Dialogs]::Press($window, $(if ($asked.Count -eq 1) { $yes } else { $no }))
            Wait-For { [Dialogs]::Find($corePid, 'Sikemux') -eq [IntPtr]::Zero } 10 | Out-Null
        }
        Start-Sleep -Milliseconds 250
    }
    if (-not $login.HasExited) { $login.Kill() }
    Stop-Job $server
    Remove-Job -Force $server
    $printed = Get-Content -Raw $said -ErrorAction SilentlyContinue
    Check '`sikemux login` shows the code to approve' ($printed -match '"code":"BCDF GHJK"') "$printed".Trim()
    Check '`sikemux login` asks once, in the core''s window' (
        $asked.Count -eq 1 -and $asked[0] -match 'Sign this computer in to your Sikemux account'
    ) "$($asked.Count) asks: $($asked -join ' | ')"
    Check '`sikemux login` signs the host in' ($printed -match '"signedIn":true' -and $login.ExitCode -eq 0) (
        "exit $($login.ExitCode)"
    )
}

# The load config the linker wrote, read from the installed file.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$dumpbin = & $vswhere -latest -products * -find 'VC\Tools\MSVC\**\dumpbin.exe' | Select-Object -First 1
$config = (& $dumpbin /nologo /loadconfig $cli | Out-String)
$config | Out-File -Encoding utf8 (Join-Path $Out 'loadconfig.txt')
$flag = [regex]::Match($config, '([0-9A-Fa-f]+)\s+Dependent Load Flag')
$flags = if ($flag.Success) { [Convert]::ToInt32($flag.Groups[1].Value, 16) } else { 0 }
Check 'the exe loads its libraries'' libraries from System32 alone' (($flags -band 0x800) -ne 0) (
    "Dependent Load Flag $(if ($flag.Success) { $flag.Groups[1].Value } else { 'absent' })"
)

try {
    $installed = Host-Command '--service', 'install'
    Check 'the service installs and starts' ($installed.Code -eq 0) $installed.Text.Trim()
    $service = Get-CimInstance Win32_Service | Where-Object { $_.PathName -like "*$install*" }
    Check 'Windows runs it as LocalSystem' ($service.StartName -eq 'LocalSystem' -and $service.State -eq 'Running') (
        "$($service.Name) $($service.State) as $($service.StartName)"
    )

    $started = Host-Command 'start'
    Check 'the service starts the core' ($started.Code -eq 0) $started.Text.Trim()
    $first = Check-Core 'first start'
    $parent = (Get-CimInstance Win32_Process -Filter "ProcessId = $($first.pid)").ParentProcessId
    Check 'the service is the core''s parent' ($parent -eq $service.ProcessId) "parent $parent, service $($service.ProcessId)"

    # Ended as the per-user crash-restart run ends it.
    & taskkill /F /PID $first.pid | Out-Null
    $deadline = (Get-Date).AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 500
        $again = Status
    } while ((Get-Date) -lt $deadline -and -not ($again.running -and $again.pid -ne $first.pid -and $again.trust))
    Check 'the service starts a new core after it is killed' ($again.running -and $again.pid -ne $first.pid) (
        "pid $($first.pid) then $($again.pid)"
    )
    $core = Check-Core 'after the restart'
    Check-Window $core
    $app = Join-Path $install 'examples\remote.exe'
    New-Item -ItemType Directory -Force -Path (Split-Path $app) | Out-Null
    Copy-Item $Remote $app
    & (Join-Path $PSScriptRoot 'windows-children.ps1') -Socket $core.socket -Probe $Probe -App $app -Out $Out
    if ($LASTEXITCODE) { $failed += 'each child the core starts runs at Medium' }
    Check-Login $core
} finally {
    $removed = Host-Command '--service', 'remove'
    Check 'the service is removed' ($removed.Code -eq 0) $removed.Text.Trim()
    Get-ChildItem $env:LOCALAPPDATA -Recurse -Filter core.log -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like '*sikemux*' } |
        ForEach-Object { Copy-Item $_.FullName (Join-Path $Out "core-$($_.Directory.Parent.Name).log") }
}

if ($failed.Count -gt 0) { throw "these checks failed: $($failed -join '; ')" }
