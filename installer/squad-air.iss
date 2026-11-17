#define MyAppName "SquadAir"
#define MyAppVersion "0.1.0"
#define MyAppPublisher "SquadAir Open Source"
#define MyAppURL "https://squad-air.local"
#define MyAppExeName "SquadAir.exe"

[Setup]
AppId={{7A9E4E82-4D2E-4D1F-9B6A-5E3C2A8B1F90}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={localappdata}\Programs\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=SquadAir-Setup-windows-x86_64
SetupIconFile=..\assets\icons\squad-air.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
WizardStyle=modern
WizardResizable=no
Compression=lzma2/normal
SolidCompression=no
CloseApplications=force
RestartApplications=no
VersionInfoVersion=0.1.0.0
VersionInfoCompany={#MyAppPublisher}
VersionInfoDescription=SquadAir Encrypted P2P Voice & Screen Share Setup
VersionInfoProductName={#MyAppName}
VersionInfoProductVersion=0.1.0.0
VersionInfoCopyright=Copyright (c) 2026 SquadAir Contributors

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\squad-client.exe"; DestDir: "{app}"; DestName: "{#MyAppExeName}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Comment: "SquadAir Encrypted P2P Voice & Screen Share"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon; Comment: "SquadAir Encrypted P2P Voice & Screen Share"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent
