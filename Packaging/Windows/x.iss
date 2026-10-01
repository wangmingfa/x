; Inno Setup installer for x (Windows).
;
; Compile locally with the Inno Setup Compiler (ISCC):
;   ISCC.exe Packaging\Windows\x.iss
; The CI release workflow compiles it automatically after a cargo build.
;
; The built binary is expected at ..\..\target\release\x.exe relative to this
; script (i.e. the workspace target directory).

#define MyAppName    "x"
#define MyAppVersion "0.1.0"
#define MyPublisher  "x contributors"
#define MyURL        "https://github.com/xsys/x"
#define SourcePath   "..\..\target\release\x.exe"

[Setup]
AppId={{A1B2C3D4-E5F6-7890-ABCD-1234567890AB}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyPublisher}
AppPublisherURL={#MyURL}
AppSupportURL={#MyURL}
AppUpdatesURL={#MyURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
OutputBaseFilename=x-{#MyAppVersion}-windows-x86_64-setup
Compression=lzma2/ultra64
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64
ChangesEnvironment=yes
PrivilegesRequired=admin
WizardStyle=modern
UninstallDisplayName={#MyAppName}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: {#SourcePath}; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\x.exe"; WorkingDir: "{userdesktop}"
Name: "{commondesktop}\{#MyAppName}"; Filename: "{app}\x.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop icon"; GroupDescription: "Additional icons:"
Name: "path";        Description: "Add {#MyAppName} to your PATH"; GroupDescription: "Environment:"

[Registry]
; Append the install dir to the system PATH only when the "path" task is selected
; and it is not already present.
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
  ValueType: expandsz; ValueName: "Path"; Tasks: path; Check: NeedsAddPath('{app}')

[Code]
function NeedsAddPath(Param: string): Boolean;
var
  OrigPath: string;
begin
  if RegQueryStringValue(HKLM, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', 'Path', OrigPath) then
  begin
    if Pos(Param, OrigPath) = 0 then
      Result := True
    else
      Result := False;
  end
  else
    Result := True;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  OrigPath: string;
  NewPath: string;
  MsgResult: DWORD;
begin
  if (CurStep = ssPostInstall) and IsTaskSelected('path') then
  begin
    if RegQueryStringValue(HKLM, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', 'Path', OrigPath) then
    begin
      NewPath := OrigPath;
      if Pos('{app}', NewPath) = 0 then
      begin
        if (Length(NewPath) > 0) and (AnsiLastChar(NewPath) <> ';') then
          NewPath := NewPath + ';';
        NewPath := NewPath + ExpandConstant('{app}');
        RegWriteStringValue(HKLM, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', 'Path', NewPath);
        SendMessageTimeout(HWND_BROADCAST, WM_SETTINGCHANGE, 0, LPARAM('Environment'), SMTO_ABORTIFHUNG, 5000, MsgResult);
      end;
    end;
  end;
end;
