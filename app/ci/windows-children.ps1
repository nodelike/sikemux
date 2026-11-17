# Has a running core start a program that saves what `whoami /groups` prints, each way the core
# starts programs, and checks every one runs at Medium. `-App` is a build of the `remote` example
# where the core takes it for its app.
param(
    [Parameter(Mandatory)] [string] $Socket,
    [Parameter(Mandatory)] [string] $Probe,
    [Parameter(Mandatory)] [string] $App,
    [Parameter(Mandatory)] [string] $Out
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $Out | Out-Null

$failed = @()
function Check([string] $What, [bool] $Held, [string] $Detail = '') {
    $line = "$(if ($Held) { 'ok  ' } else { 'FAIL' }) $What$(if ($Detail) { ": $Detail" })"
    $line | Out-File -Encoding utf8 -Append (Join-Path $Out 'checks.txt')
    Write-Host $line
    if (-not $Held) { $script:failed += $What }
}

function Wait-For([scriptblock] $Ready, [int] $Seconds) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while (-not ($found = & $Ready) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 250 }
    $found
}

$children = Join-Path $env:TEMP 'children'
New-Item -ItemType Directory -Force -Path $children | Out-Null
$reporter = Join-Path $children 'probe.exe'
Copy-Item $Probe $reporter
$repo = Join-Path $children 'repo'
& git init -q -b main $repo
& git -C $repo -c user.name=proof -c user.email=proof@localhost commit -q --allow-empty -m first
& git -C $repo branch other
$hook = "#!/bin/sh`nexec '$($reporter -replace '\\', '/')' git`n"
[IO.File]::WriteAllText((Join-Path $repo '.git\hooks\post-checkout'), $hook)
& icacls $children /setowner $env:USERNAME /T /Q | Out-Null

$ErrorActionPreference = 'Continue'
(& $App mac $Socket children $reporter $repo 2>&1 | Out-String) |
    Out-File -Encoding utf8 (Join-Path $Out 'children.txt')
$ErrorActionPreference = 'Stop'

$ways = [ordered]@{
    terminal = 'a program a terminal''s shell starts'
    git = 'a hook git runs for the core'
}
$agent = { Get-ChildItem $children -Filter *.txt | Where-Object { -not $ways.Contains($_.BaseName) } }
Wait-For { ($ways.Keys | Where-Object { Test-Path (Join-Path $children "$_.txt") }).Count -eq $ways.Count -and (& $agent) } 60 | Out-Null
$reports = @($ways.Keys | ForEach-Object { [pscustomobject]@{ What = $ways[$_]; File = Join-Path $children "$_.txt" } })
$reports += @(& $agent | ForEach-Object { [pscustomobject]@{ What = "an agent's program, run as $($_.BaseName)"; File = $_.FullName } })
if (-not (& $agent)) { $reports += [pscustomobject]@{ What = 'an agent''s program'; File = Join-Path $children 'agent.txt' } }
foreach ($report in $reports) {
    $text = Get-Content -Raw $report.File -ErrorAction SilentlyContinue
    if ($text) { $text | Out-File -Encoding utf8 (Join-Path $Out "whoami-$(Split-Path -Leaf $report.File)") }
    $level = [regex]::Match("$text", 'Mandatory Label\\(\w+) Mandatory Level').Groups[1].Value
    Check "$($report.What) runs at Medium" ($level -eq 'Medium') "$(if ($text) { $level } else { 'it never ran' })"
}
if ($failed.Count -gt 0) { exit 1 }
