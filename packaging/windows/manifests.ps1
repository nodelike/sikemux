# Writes the winget and Scoop manifests of a release from its two installers, ready to submit.
param(
  [Parameter(Mandatory)] [string] $Version,
  [Parameter(Mandatory)] [string] $X64,
  [Parameter(Mandatory)] [string] $Arm64,
  [Parameter(Mandatory)] [string] $Out,
  [ValidateSet("machine", "user")] [string] $Scope = "machine",
  [string] $Base = "https://github.com/nodelike/sikemux/releases/download/v$Version"
)
$ErrorActionPreference = "Stop"

$Id = "Nodelike.Sikemux"
$ProductCode = "{6D6F1A52-3B1C-4B7E-9C55-5A3E2D8B7C41}_is1"
$Description = "Own your software factory: agents build and ship, you make the calls"
$Installers = [ordered]@{
  x64 = @{ File = $X64; Url = "$Base/$(Split-Path $X64 -Leaf)"; Hash = (Get-FileHash $X64 -Algorithm SHA256).Hash }
  arm64 = @{ File = $Arm64; Url = "$Base/$(Split-Path $Arm64 -Leaf)"; Hash = (Get-FileHash $Arm64 -Algorithm SHA256).Hash }
}

$Winget = Join-Path $Out "winget\manifests\n\Nodelike\Sikemux\$Version"
New-Item -ItemType Directory -Force $Winget | Out-Null
$Header = "ManifestVersion: 1.9.0"
@"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.1.9.0.schema.json
PackageIdentifier: $Id
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
$Header
"@ | Set-Content -Encoding utf8NoBOM "$Winget\$Id.yaml"

$Entries = foreach ($Arch in $Installers.Keys) {
  @"
  - Architecture: $Arch
    InstallerUrl: $($Installers[$Arch].Url)
    InstallerSha256: $($Installers[$Arch].Hash)
"@
}
@"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.1.9.0.schema.json
PackageIdentifier: $Id
PackageVersion: $Version
InstallerType: inno
Scope: $Scope
MinimumOSVersion: 10.0.19045.0
UpgradeBehavior: install
ProductCode: '$ProductCode'
Installers:
$($Entries -join "`n")
ManifestType: installer
$Header
"@ | Set-Content -Encoding utf8NoBOM "$Winget\$Id.installer.yaml"

@"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.1.9.0.schema.json
PackageIdentifier: $Id
PackageVersion: $Version
PackageLocale: en-US
Publisher: Sikemux
PublisherUrl: https://sikemux.com
PublisherSupportUrl: https://github.com/nodelike/sikemux/issues
PrivacyUrl: https://sikemux.com/privacy
PackageName: Sikemux
PackageUrl: https://sikemux.com
License: MIT
LicenseUrl: https://github.com/nodelike/sikemux/blob/main/LICENSE
ShortDescription: $Description
Moniker: sikemux
Tags:
  - ai
  - coding-agents
  - terminal
ReleaseNotesUrl: https://github.com/nodelike/sikemux/releases/tag/v$Version
ManifestType: defaultLocale
$Header
"@ | Set-Content -Encoding utf8NoBOM "$Winget\$Id.locale.en-US.yaml"

# Scoop unpacks the installer rather than running it, so a Scoop install has no uninstaller and
# never updates itself; Scoop updates it.
$Scoop = Join-Path $Out "scoop"
New-Item -ItemType Directory -Force $Scoop | Out-Null
$AutoUrl = "https://github.com/nodelike/sikemux/releases/download/v`$version/Sikemux_`$version"
[ordered]@{
  version = $Version
  description = $Description
  homepage = "https://sikemux.com"
  license = "MIT"
  architecture = [ordered]@{
    "64bit" = [ordered]@{ url = $Installers.x64.Url; hash = $Installers.x64.Hash.ToLower() }
    arm64 = [ordered]@{ url = $Installers.arm64.Url; hash = $Installers.arm64.Hash.ToLower() }
  }
  innosetup = $true
  bin = @("sikemux.exe")
  shortcuts = @(, @("sikemux.exe", "Sikemux"))
  checkver = [ordered]@{ github = "https://github.com/nodelike/sikemux" }
  autoupdate = [ordered]@{
    architecture = [ordered]@{
      "64bit" = @{ url = "${AutoUrl}_x64-setup.exe" }
      arm64 = @{ url = "${AutoUrl}_arm64-setup.exe" }
    }
  }
} | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8NoBOM "$Scoop\sikemux.json"

Get-ChildItem -Recurse -File $Out | ForEach-Object { $_.FullName }
