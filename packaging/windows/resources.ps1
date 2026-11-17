# Writes the icon and the version resources linked into sikemux.exe and sikemux-host.exe, so
# every exe names the product and one version.
param(
  [Parameter(Mandatory)][string]$Version,
  [Parameter(Mandatory)][string]$Out
)
$ErrorActionPreference = "Stop"
$Root = Resolve-Path "$PSScriptRoot\..\.."

if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)(-nightly\.(\d+))?$') {
  throw "$Version is neither a stable nor a nightly version"
}
$Build = if ($Matches[5]) { $Matches[5] } else { "0" }
$Numeric = "$($Matches[1]),$($Matches[2]),$($Matches[3]),$Build"
New-Item -ItemType Directory -Force $Out | Out-Null

# rc.exe takes a PNG frame only at 256 px, so the smaller Linux icons go in as 32-bit
# bitmaps: rows bottom-up, then an empty mask, with the height counted twice.
function Bitmap-Frame($Path) {
  Add-Type -AssemblyName System.Drawing
  $Image = [Drawing.Bitmap]::new($Path)
  $Side = $Image.Width
  $Data = $Image.LockBits([Drawing.Rectangle]::new(0, 0, $Side, $Side),
    [Drawing.Imaging.ImageLockMode]::ReadOnly, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $Pixels = New-Object byte[] ($Side * $Side * 4)
  [Runtime.InteropServices.Marshal]::Copy($Data.Scan0, $Pixels, 0, $Pixels.Length)
  $Image.UnlockBits($Data); $Image.Dispose()
  $Frame = New-Object IO.MemoryStream
  $Bits = New-Object IO.BinaryWriter $Frame
  $Bits.Write([uint32]40); $Bits.Write([int32]$Side); $Bits.Write([int32]($Side * 2))
  $Bits.Write([uint16]1); $Bits.Write([uint16]32); $Bits.Write([uint32]0)
  $Bits.Write([uint32]0); $Bits.Write([int32]0); $Bits.Write([int32]0)
  $Bits.Write([uint32]0); $Bits.Write([uint32]0)
  for ($Row = $Side - 1; $Row -ge 0; $Row--) { $Bits.Write($Pixels, $Row * $Side * 4, $Side * 4) }
  $Bits.Write((New-Object byte[] ([Math]::Ceiling($Side / 32) * 4 * $Side)))
  , $Frame.ToArray()
}

$Sizes = 32, 128, 256
$Frames = $Sizes | ForEach-Object {
  $Png = "$Root\brand\linux\icon-$_.png"
  if ($_ -ge 256) { , [IO.File]::ReadAllBytes($Png) } else { , (Bitmap-Frame $Png) }
}
$Icon = New-Object IO.MemoryStream
$Writer = New-Object IO.BinaryWriter $Icon
$Writer.Write([uint16]0); $Writer.Write([uint16]1); $Writer.Write([uint16]$Frames.Count)
$Offset = 6 + 16 * $Frames.Count
for ($i = 0; $i -lt $Frames.Count; $i++) {
  $Side = if ($Sizes[$i] -ge 256) { 0 } else { $Sizes[$i] }
  $Writer.Write([byte]$Side); $Writer.Write([byte]$Side); $Writer.Write([byte]0); $Writer.Write([byte]0)
  $Writer.Write([uint16]1); $Writer.Write([uint16]32)
  $Writer.Write([uint32]$Frames[$i].Length); $Writer.Write([uint32]$Offset)
  $Offset += $Frames[$i].Length
}
$Frames | ForEach-Object { $Writer.Write([byte[]]$_) }
[IO.File]::WriteAllBytes("$Out\sikemux.ico", $Icon.ToArray())

$Rc = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\rc.exe" |
  Sort-Object FullName -Descending | Select-Object -First 1
if (-not $Rc) { throw "rc.exe from the Windows SDK is missing" }

$Programs = [ordered]@{ "sikemux" = "Sikemux"; "sikemux-host" = "Sikemux Core" }
foreach ($Name in $Programs.Keys) {
  @"
1 ICON "sikemux.ico"
1 VERSIONINFO
FILEVERSION $Numeric
PRODUCTVERSION $Numeric
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Sikemux"
      VALUE "FileDescription", "$($Programs[$Name])"
      VALUE "FileVersion", "$Version"
      VALUE "LegalCopyright", "Copyright (c) Sikemux contributors"
      VALUE "OriginalFilename", "$Name.exe"
      VALUE "ProductName", "Sikemux"
      VALUE "ProductVersion", "$Version"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"@ | Set-Content -Encoding ascii "$Out\$Name.rc"
  & $Rc.FullName /nologo /fo "$Out\$Name.res" "$Out\$Name.rc"
  if ($LASTEXITCODE -ne 0) { throw "rc.exe failed on $Name.rc" }
}
