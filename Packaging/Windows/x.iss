; Inno Setup installer for x (Windows).
;
; Compile locally with the Inno Setup Compiler (ISCC):
;   ISCC.exe Packaging\Windows\x.iss
; The CI release workflow compiles it automatically after a cargo build, passing
; /DMyAppVersion and /DReleaseTag so the file it produces is named after the
; release it belongs to; the fallbacks below are what a local compile gets.
;
; The built binary is expected at ..\..\target\release\x.exe relative to this
; script (i.e. the workspace target directory).

#define MyAppName    "x"
#define MyPublisher  "x contributors"
#define MyURL        "https://github.com/xsys/x"
#define SourcePath   "..\..\target\release\x.exe"

; The file name comes from the git tag, not from MyAppVersion, so the asset the
; workflow uploads is spelled exactly as the release notes spell it. MyAppVersion
; carries the same number without the leading "v", because that is what
; AppVersion is meant to show. The fallbacks are for local compiles only.
#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif
#ifndef ReleaseTag
  #define ReleaseTag "v0.1.0"
#endif

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
OutputBaseFilename=x-{#ReleaseTag}-windows-x86_64-setup
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

[Code]
{ PATH is written here, not from a [Registry] entry: an entry with a ValueName but }
{ no ValueData creates the key and writes no value, so the two would be a silent  }
{ second route through the same registry value.                                  }
procedure CurStepChanged(CurStep: TSetupStep);
var
  OrigPath: string;
  NewPath: string;
  AppDir: string;
begin
  if (CurStep = ssPostInstall) and IsTaskSelected('path') then
  begin
    if RegQueryStringValue(HKLM, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', 'Path', OrigPath) then
    begin
      AppDir := ExpandConstant('{app}');
      NewPath := OrigPath;
      if Pos(AppDir, NewPath) = 0 then
      begin
        if (Length(NewPath) > 0) and (Copy(NewPath, Length(NewPath), 1) <> ';') then
          NewPath := NewPath + ';';
        NewPath := NewPath + AppDir;
        RegWriteStringValue(HKLM, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', 'Path', NewPath);
      end;
    end;
  end;
end;
