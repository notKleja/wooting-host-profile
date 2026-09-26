#ifndef AppSource
  #error AppSource must point to the published Windows application directory.
#endif

#ifndef OutputDirectory
  #define OutputDirectory "."
#endif

#define AppName "Wooting Switch"
#define AppVersion "0.5.0"
#define AppPublisher "notKleja"
#define AppURL "https://github.com/notKleja/wooting-host-profile"
#define AppExeName "WootingHostProfile.WinUI.exe"

[Setup]
AppId={{26D1F80B-8E54-4BC6-BFE0-F8974929CD1B}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/wiki
AppUpdatesURL={#AppURL}/releases
DefaultDirName={localappdata}\WootingHostProfile
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDirectory}
OutputBaseFilename=Wooting-Switch-Setup-Windows-x64
SetupIconFile=..\..\windows\WootingHostProfile.WinUI\Assets\AppIcon.ico
UninstallDisplayIcon={app}\{#AppExeName}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
MinVersion=10.0.17763
VersionInfoVersion={#AppVersion}.0
VersionInfoCompany={#AppPublisher}
VersionInfoDescription={#AppName} installer
VersionInfoProductName={#AppName}
VersionInfoProductVersion={#AppVersion}

[Files]
Source: "{#AppSource}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Wooting Switch"; Flags: uninsdeletevalue dontcreatekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Wooting Host Profile"; Flags: uninsdeletevalue dontcreatekey

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Open {#AppName}"; Flags: nowait postinstall skipifsilent
