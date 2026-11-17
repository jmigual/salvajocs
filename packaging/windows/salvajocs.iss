; Per-user installer for salvajocs. Build with:
;   iscc /DAppVersion=0.1.0 /DSourceExe=C:\path\to\salvajocs.exe /O<output dir> salvajocs.iss
; The in-app updater depends on the output name, the install folder and the /RELAUNCH switch.

#ifndef AppVersion
  #error Pass the version with /DAppVersion=<x.y.z>.
#endif
#ifndef SourceExe
  #error Pass the built executable with /DSourceExe=<path>.
#endif

[Setup]
; Fixed GUID: a new installer with the same AppId upgrades the existing install.
AppId={{01BA2E00-9B15-4204-932E-0C09672F0E5A}
AppName=salvajocs
AppVersion={#AppVersion}
AppPublisher=salvajocs
AppPublisherURL=https://github.com/jmigual/salvajocs
DefaultDirName={localappdata}\Programs\salvajocs
DisableProgramGroupPage=yes
DisableDirPage=yes
DisableReadyPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Close a running salvajocs before replacing it; /RELAUNCH below starts it again.
CloseApplications=yes
CloseApplicationsFilter=salvajocs.exe
RestartApplications=no
SetupIconFile=..\..\crates\salvajocs-gui\assets\salvajocs.ico
UninstallDisplayIcon={app}\salvajocs.ico
OutputBaseFilename=salvajocs-{#AppVersion}-x86_64-setup
WizardStyle=modern
Compression=lzma2
SolidCompression=yes

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "salvajocs.exe"; Flags: ignoreversion
Source: "..\..\crates\salvajocs-gui\assets\salvajocs.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\crates\salvajocs-gui\assets\fonts\OFL.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\salvajocs"; Filename: "{app}\salvajocs.exe"; IconFilename: "{app}\salvajocs.ico"

; Only program files are installed here, so only they are removed on uninstall. The
; configuration, cache and backups live elsewhere and are left alone.
[Run]
; Interactive install: the usual checkbox on the last page.
Filename: "{app}\salvajocs.exe"; Description: "Launch salvajocs"; Flags: nowait postinstall skipifsilent; Check: not Relaunch
; Silent update from the app: start it again, also with /VERYSILENT.
Filename: "{app}\salvajocs.exe"; Flags: nowait; Check: Relaunch

[Code]
{ True when Setup was started with the custom /RELAUNCH switch. }
function Relaunch: Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/RELAUNCH') = 0 then
      Result := True;
end;
