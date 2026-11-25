; LabelCraft installer for Windows (Inno Setup 6).
; Installs for the current user only, into %LOCALAPPDATA%\Programs\LabelCraft, so no
; admin rights are needed. Adds a Start menu shortcut, an optional desktop shortcut and an
; optional "Open with" entry for PDF and image files. Uninstall from Settings → Apps.
;
; Build: iscc /DAppVersion=0.6.1 /DSourceExe=path\to\labelcraft.exe packaging\windows\labelcraft.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\labelcraft.exe"
#endif

[Setup]
AppId={{6B0E2F31-7C4A-4D8B-9E7F-2A1C5D3B9F40}
AppName=LabelCraft
AppVersion={#AppVersion}
AppVerName=LabelCraft {#AppVersion}
AppPublisher=civarson
AppPublisherURL=https://github.com/civarson/labelcraft
AppSupportURL=https://github.com/civarson/labelcraft/issues
AppUpdatesURL=https://github.com/civarson/labelcraft/releases
DefaultDirName={localappdata}\Programs\LabelCraft
DisableDirPage=yes
DisableProgramGroupPage=yes
DefaultGroupName=LabelCraft
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputBaseFilename=labelcraft-windows-x64-setup
OutputDir=..\..\dist
SetupIconFile=labelcraft.ico
UninstallDisplayIcon={app}\LabelCraft.exe
UninstallDisplayName=LabelCraft
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
ChangesAssociations=yes

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"
Name: "openwith"; Description: "Add LabelCraft to ""Open with"" for PDF, PNG and JPG files"; GroupDescription: "Files:"

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "LabelCraft.exe"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\LabelCraft"; Filename: "{app}\LabelCraft.exe"; Comment: "Crisp shipping labels, ready to print"
Name: "{userdesktop}\LabelCraft"; Filename: "{app}\LabelCraft.exe"; Comment: "Crisp shipping labels, ready to print"; Tasks: desktopicon

[Registry]
; Same keys as Settings → About → Add to "Open with", removed again on uninstall.
Root: HKCU; Subkey: "Software\Classes\LabelCraft.Label"; ValueType: string; ValueName: ""; ValueData: "Shipping label (LabelCraft)"; Flags: uninsdeletekey; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\LabelCraft.Label\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\LabelCraft.exe"",0"; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\LabelCraft.Label\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\LabelCraft.exe"" ""%1"""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Applications\LabelCraft.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "LabelCraft"; Flags: uninsdeletekey; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Applications\LabelCraft.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\LabelCraft.exe"" ""%1"""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Applications\LabelCraft.exe\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Applications\LabelCraft.exe\SupportedTypes"; ValueType: string; ValueName: ".png"; ValueData: ""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Applications\LabelCraft.exe\SupportedTypes"; ValueType: string; ValueName: ".jpg"; ValueData: ""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Applications\LabelCraft.exe\SupportedTypes"; ValueType: string; ValueName: ".jpeg"; ValueData: ""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: none; ValueName: "LabelCraft.Label"; Flags: uninsdeletevalue; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\.png\OpenWithProgids"; ValueType: none; ValueName: "LabelCraft.Label"; Flags: uninsdeletevalue; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\.jpg\OpenWithProgids"; ValueType: none; ValueName: "LabelCraft.Label"; Flags: uninsdeletevalue; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\.jpeg\OpenWithProgids"; ValueType: none; ValueName: "LabelCraft.Label"; Flags: uninsdeletevalue; Tasks: openwith

[Run]
Filename: "{app}\LabelCraft.exe"; Description: "Open LabelCraft now"; Flags: nowait postinstall skipifsilent
