; Inno Setup script. Build with: ISCC /DAppVersion=<version> installer\windows\screentint.iss
#define AppName "ScreenTint"
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{B62AD660-2982-44EF-A334-C8E7963A3ECA}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=tulerws
AppPublisherURL=https://github.com/tulerws/ScreenTint
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; per-user install by default (no UAC); the user can choose "all users" in the dialog
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=ScreenTint-{#AppVersion}-windows-x64-setup
SetupIconFile=..\..\assets\icon.ico
UninstallDisplayIcon={app}\ScreenTint.exe
LicenseFile=..\..\LICENSE
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"

[Tasks]
Name: "autostart"; Description: "Start ScreenTint when I sign in"; Flags: unchecked

[Files]
Source: "..\..\target\release\ScreenTint.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\ScreenTint.exe"

[Registry]
; created by the "autostart" task
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "ScreenTint"; ValueData: """{app}\ScreenTint.exe"""; Tasks: autostart
; the app can also create this value itself, so always remove it on uninstall
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "ScreenTint"; Flags: dontcreatekey uninsdeletevalue

[Run]
Filename: "{app}\ScreenTint.exe"; Description: "Launch ScreenTint"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM ScreenTint.exe"; Flags: runhidden; RunOnceId: "StopScreenTint"
