; ShippingLabelCraft installer for Windows (Inno Setup 6).
; Installs for the current user only, into %LOCALAPPDATA%\Programs\ShippingLabelCraft, so no
; admin rights are needed. Adds a Start menu shortcut, an optional desktop shortcut and an
; optional "Open with" entry for PDF and image files. Uninstall from Settings → Apps.
;
; Build: iscc /DAppVersion=0.6.1 /DSourceExe=path\to\shippinglabelcraft.exe packaging\windows\shippinglabelcraft.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\shippinglabelcraft.exe"
#endif

[Setup]
AppId={{6B0E2F31-7C4A-4D8B-9E7F-2A1C5D3B9F40}
AppName=ShippingLabelCraft
AppVersion={#AppVersion}
AppVerName=ShippingLabelCraft {#AppVersion}
AppPublisher=civarson
AppPublisherURL=https://github.com/civarson/shippinglabelcraft
AppSupportURL=https://github.com/civarson/shippinglabelcraft/issues
AppUpdatesURL=https://github.com/civarson/shippinglabelcraft/releases
DefaultDirName={localappdata}\Programs\ShippingLabelCraft
DisableDirPage=yes
DisableProgramGroupPage=yes
DefaultGroupName=ShippingLabelCraft
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputBaseFilename=shippinglabelcraft-windows-x64-setup
OutputDir=..\..\dist
SetupIconFile=shippinglabelcraft.ico
UninstallDisplayIcon={app}\ShippingLabelCraft.exe
UninstallDisplayName=ShippingLabelCraft
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
ChangesAssociations=yes

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"
Name: "openwith"; Description: "Add ShippingLabelCraft to ""Open with"" for PDF, PNG and JPG files"; GroupDescription: "Files:"

[InstallDelete]
; Leftovers from before the rename (LabelCraft).
Type: files; Name: "{app}\LabelCraft.exe"
Type: files; Name: "{userprograms}\LabelCraft.lnk"
Type: files; Name: "{userdesktop}\LabelCraft.lnk"

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "ShippingLabelCraft.exe"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\ShippingLabelCraft"; Filename: "{app}\ShippingLabelCraft.exe"; Comment: "Crisp shipping labels, ready to print"
Name: "{userdesktop}\ShippingLabelCraft"; Filename: "{app}\ShippingLabelCraft.exe"; Comment: "Crisp shipping labels, ready to print"; Tasks: desktopicon

[Registry]
; Remove the pre-rename "Open with" keys.
Root: HKCU; Subkey: "Software\Classes\LabelCraft.Label"; ValueType: none; Flags: deletekey
Root: HKCU; Subkey: "Software\Classes\Applications\LabelCraft.exe"; ValueType: none; Flags: deletekey
Root: HKCU; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: none; ValueName: "LabelCraft.Label"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.png\OpenWithProgids"; ValueType: none; ValueName: "LabelCraft.Label"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.jpg\OpenWithProgids"; ValueType: none; ValueName: "LabelCraft.Label"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.jpeg\OpenWithProgids"; ValueType: none; ValueName: "LabelCraft.Label"; Flags: deletevalue
; Same keys as Settings → About → Add to "Open with", removed again on uninstall.
Root: HKCU; Subkey: "Software\Classes\ShippingLabelCraft.Label"; ValueType: string; ValueName: ""; ValueData: "Shipping label (ShippingLabelCraft)"; Flags: uninsdeletekey; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\ShippingLabelCraft.Label\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\ShippingLabelCraft.exe"",0"; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\ShippingLabelCraft.Label\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\ShippingLabelCraft.exe"" ""%1"""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Applications\ShippingLabelCraft.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "ShippingLabelCraft"; Flags: uninsdeletekey; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Applications\ShippingLabelCraft.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\ShippingLabelCraft.exe"" ""%1"""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Applications\ShippingLabelCraft.exe\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Applications\ShippingLabelCraft.exe\SupportedTypes"; ValueType: string; ValueName: ".png"; ValueData: ""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Applications\ShippingLabelCraft.exe\SupportedTypes"; ValueType: string; ValueName: ".jpg"; ValueData: ""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Applications\ShippingLabelCraft.exe\SupportedTypes"; ValueType: string; ValueName: ".jpeg"; ValueData: ""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: none; ValueName: "ShippingLabelCraft.Label"; Flags: uninsdeletevalue; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\.png\OpenWithProgids"; ValueType: none; ValueName: "ShippingLabelCraft.Label"; Flags: uninsdeletevalue; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\.jpg\OpenWithProgids"; ValueType: none; ValueName: "ShippingLabelCraft.Label"; Flags: uninsdeletevalue; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\.jpeg\OpenWithProgids"; ValueType: none; ValueName: "ShippingLabelCraft.Label"; Flags: uninsdeletevalue; Tasks: openwith

[Run]
Filename: "{app}\ShippingLabelCraft.exe"; Description: "Open ShippingLabelCraft now"; Flags: nowait postinstall skipifsilent
