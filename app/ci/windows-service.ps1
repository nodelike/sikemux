# Installs the Sikemux service from a folder only administrators write, has it start this person's
# core, and checks what Windows and the core report about that core. Needs an elevated runner.
param(
    [Parameter(Mandatory)] [string] $Built,
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
    Check-Core 'after the restart' | Out-Null
} finally {
    $removed = Host-Command '--service', 'remove'
    Check 'the service is removed' ($removed.Code -eq 0) $removed.Text.Trim()
    Get-ChildItem $env:LOCALAPPDATA -Recurse -Filter core.log -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like '*sikemux*' } |
        ForEach-Object { Copy-Item $_.FullName (Join-Path $Out "core-$($_.Directory.Parent.Name).log") }
}

if ($failed.Count -gt 0) { throw "these checks failed: $($failed -join '; ')" }
