; Ravune's Windows installer (Inno Setup 6). The release workflow builds it:
;   iscc /DVersion=0.9.3-beta installer\ravune.iss   (after cargo build --release)
; → dist\ravune-<version>-windows-x64-setup.exe

#ifndef Version
  #define Version "0.0.0"
#endif

[Setup]
AppId={{8CFF242B-1254-48CD-8CBE-573A303FE29E}
AppName=Ravune
AppVersion={#Version}
AppVerName=Ravune {#Version}
AppPublisher=The Ravune contributors
AppPublisherURL=https://g1lg1l.github.io/ravune/
AppSupportURL=https://github.com/g1lg1l/ravune/issues
; for this user by default (AppData\Local\Programs, no admin): Ravune can then update itself
; by swapping its files. "All users" (Program Files) stays a choice; those copies update by
; running the next installer.
DefaultDirName={autopf}\Ravune
DefaultGroupName=Ravune
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\LICENSE-MIT
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\ravune.exe
OutputDir=..\dist
OutputBaseFilename=ravune-{#Version}-windows-x64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "association"; Description: "Open .ravune projects with Ravune"

[Files]
Source: "..\target\release\ravune.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\ravune-host.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\ravune-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
; Ravune was Phasor until 0.9: a copy installed then keeps its phasor*.exe (its shortcuts and
; the 0.8 updater open those), now running Ravune
Source: "..\target\release\ravune.exe"; DestDir: "{app}"; DestName: "phasor.exe"; Flags: ignoreversion; Check: HadPhasor
Source: "..\target\release\ravune-host.exe"; DestDir: "{app}"; DestName: "phasor-host.exe"; Flags: ignoreversion; Check: HadPhasor
Source: "..\target\release\ravune-mcp.exe"; DestDir: "{app}"; DestName: "phasor-mcp.exe"; Flags: ignoreversion; Check: HadPhasor
Source: "..\README.md"; DestDir: "{app}"
Source: "..\LICENSE-MIT"; DestDir: "{app}"
Source: "..\LICENSE-APACHE"; DestDir: "{app}"

[Icons]
Name: "{autoprograms}\Ravune"; Filename: "{app}\ravune.exe"
Name: "{autodesktop}\Ravune"; Filename: "{app}\ravune.exe"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\.ravune"; ValueType: string; ValueName: ""; ValueData: "Ravune.Project"; Flags: uninsdeletevalue; Tasks: association
Root: HKA; Subkey: "Software\Classes\Ravune.Project"; ValueType: string; ValueName: ""; ValueData: "Ravune project"; Flags: uninsdeletekey; Tasks: association
Root: HKA; Subkey: "Software\Classes\Ravune.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\ravune.exe,0"; Tasks: association
Root: HKA; Subkey: "Software\Classes\Ravune.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\ravune.exe"" ""%1"""; Tasks: association

[Run]
Filename: "{app}\ravune.exe"; Description: "{cm:LaunchProgram,Ravune}"; Flags: nowait postinstall skipifsilent

[Code]
function HadPhasor: Boolean;
begin
  Result := FileExists(ExpandConstant('{app}\phasor.exe'));
end;
