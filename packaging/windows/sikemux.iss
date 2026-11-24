; Sikemux for Windows: one install per machine, with one administrator prompt. Its service keeps
; secrets, starts each person's core and installs updates with no further prompt.
; build.ps1 passes Version, NumericVersion, Arch, Stage and Output.

#define Name "Sikemux"
#define Exe "sikemux.exe"
#define Core "sikemux-host.exe"
#define PrivacyUrl "https://sikemux.com/privacy"

[Setup]
; One id per product for good: it is what ties an update and the uninstaller to an install.
AppId={{6D6F1A52-3B1C-4B7E-9C55-5A3E2D8B7C41}
AppName={#Name}
AppVersion={#Version}
AppVerName={#Name} {#Version}
AppPublisher=Sikemux
AppPublisherURL=https://sikemux.com
AppSupportURL=https://github.com/nodelike/sikemux/issues
AppUpdatesURL=https://sikemux.com
VersionInfoVersion={#NumericVersion}
VersionInfoProductName={#Name}
VersionInfoProductVersion={#NumericVersion}
VersionInfoProductTextVersion={#Version}
VersionInfoDescription={#Name} installer
PrivilegesRequired=admin
DefaultDirName={autopf}\{#Name}
DisableDirPage=auto
DisableProgramGroupPage=yes
DisableReadyPage=yes
WizardStyle=modern
MinVersion=10.0.19045
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
SourceDir={#Stage}
OutputDir={#Output}
OutputBaseFilename=Sikemux_{#Version}_{#Arch}-setup
SetupIconFile=sikemux.ico
UninstallDisplayIcon={app}\{#Exe}
UninstallDisplayName={#Name}
Compression=lzma2/ultra64
SolidCompression=yes
; Restart Manager would close every program with one of our files open, such as a virus scanner.
; PrepareToInstall closes ours alone, and after an update the service opens the app again.
CloseApplications=no
RestartApplications=no

[Tasks]
Name: shareusage; Description: "Share anonymous usage data and crash reports (read the privacy policy: {#PrivacyUrl})"

[Files]
Source: "{#Exe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Core}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#Name}"; Filename: "{app}\{#Exe}"; AppUserModelID: "com.nodelike.sikemux"

[Run]
Filename: "{app}\{#Core}"; Parameters: "--service install"; StatusMsg: "Starting the Sikemux service..."; Flags: runhidden waituntilterminated
Filename: "{app}\{#Exe}"; Description: "Open {#Name}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallRun]
Filename: "{app}\{#Core}"; Parameters: "--service remove"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveService"
Filename: "{app}\{#Core}"; Parameters: "disable"; Flags: runhidden waituntilterminated; RunOnceId: "ForgetSignIn"
Filename: "{app}\{#Core}"; Parameters: "stop"; Flags: runhidden waituntilterminated; RunOnceId: "StopCore"
Filename: "{code:PowerShell}"; Parameters: "{code:CloseSikemux}"; Flags: runhidden waituntilterminated; RunOnceId: "StopSikemux"

[UninstallDelete]
; The `sikemux` command the app copies for its agents is the app's, not the person's data.
Type: filesandordirs; Name: "{userappdata}\sikemux\bin"
Type: dirifempty; Name: "{userappdata}\sikemux"
Type: dirifempty; Name: "{app}"

[Code]
const
  WebView2Client = 'Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  WebView2Bootstrapper = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703';

var
  PrivacyPage: TOutputMsgWizardPage;

function PresentVersion(Root: Integer; Key: String): Boolean;
var
  Version: String;
begin
  Result := RegQueryStringValue(Root, Key, 'pv', Version) and (Version <> '') and (Version <> '0.0.0.0');
end;

function WebView2Present(): Boolean;
begin
  Result := PresentVersion(HKLM, 'SOFTWARE\WOW6432Node\' + WebView2Client)
    or PresentVersion(HKLM, 'SOFTWARE\' + WebView2Client)
    or PresentVersion(HKCU, 'Software\' + WebView2Client);
end;

procedure InstallWebView2();
var
  Bootstrapper: String;
  Code: Integer;
begin
  Bootstrapper := ExpandConstant('{tmp}\MicrosoftEdgeWebview2Setup.exe');
  WizardForm.StatusLabel.Caption := 'Installing the Microsoft Edge WebView2 Runtime...';
  try
    DownloadTemporaryFile(WebView2Bootstrapper, 'MicrosoftEdgeWebview2Setup.exe', '', nil);
    Exec(Bootstrapper, '/silent /install', '', SW_HIDE, ewWaitUntilTerminated, Code);
  except
    Log('WebView2 runtime not installed: ' + GetExceptionMessage);
  end;
  if not WebView2Present() then
    Log('WebView2 runtime is still missing; the browser pane will say so');
end;

procedure InitializeWizard();
begin
  PrivacyPage := CreateOutputMsgPage(wpWelcome, 'Privacy',
    'What Sikemux sends, and how to turn it off',
    'Sikemux can send anonymous usage data and crash reports, and nothing you write or open. ' +
    'The privacy policy is at {#PrivacyUrl}.' + #13#10#13#10 +
    'Untick "Share anonymous usage data and crash reports" on the next page to turn both off. ' +
    'You can change it later in Settings.');
end;

function Preferences(): String;
begin
  Result := ExpandConstant('{localappdata}\sikemux\preferences.json');
end;

{ First install only: an existing preferences file is the person's, and Settings changes it. }
procedure WriteFirstPreferences();
var
  Fields: String;
begin
  if FileExists(Preferences()) then
    exit;
  Fields := '';
  if not WizardIsTaskSelected('shareusage') then
    Fields := '"shareUsageData":false';
  if Pos('-nightly.', '{#Version}') > 0 then begin
    if Fields <> '' then
      Fields := Fields + ',';
    Fields := Fields + '"updateChannel":"nightly"';
  end;
  if Fields = '' then
    exit;
  ForceDirectories(ExtractFileDir(Preferences()));
  SaveStringToFile(Preferences(), '{' + Fields + '}', False);
end;

function PowerShell(Param: String): String;
begin
  Result := ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe');
end;

{ Looks again after each round: until the app closes, it may start a new core. }
function CloseSikemux(Param: String): String;
begin
  Result := '-NoProfile -Command "1..3 | ForEach-Object { Get-Process | Where-Object { $_.Path -like ''' +
    ExpandConstant('{app}') + '\*'' } | Stop-Process -Force -PassThru | Wait-Process -Timeout 10 }"';
end;

{ An update stops the service, the core, the process that restarts it and the app, so none holds a file being replaced. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Result := '';
  if FileExists(ExpandConstant('{app}\{#Core}')) then begin
    Exec(ExpandConstant('{app}\{#Core}'), '--service stop', '', SW_HIDE, ewWaitUntilTerminated, Code);
    Exec(ExpandConstant('{app}\{#Core}'), 'stop', '', SW_HIDE, ewWaitUntilTerminated, Code);
    Exec(PowerShell(''), CloseSikemux(''), '', SW_HIDE, ewWaitUntilTerminated, Code);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssInstall) and not WebView2Present() then
    InstallWebView2();
  if CurStep = ssPostInstall then
    WriteFirstPreferences();
end;
