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
#define MyURL        "https://github.com/wangmingfa/x"
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
DefaultDirName={localappdata}\{#MyAppName}
DefaultGroupName={#MyAppName}
OutputBaseFilename=x-{#ReleaseTag}-windows-x86_64-setup
Compression=lzma2/ultra64
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64
ChangesEnvironment=yes
PrivilegesRequired=lowest
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
Name: "path";        Description: "Add {#MyAppName} to your PATH"; GroupDescription: "Environment:"; Flags: checkedonce

[Code]
{ PATH is written to the per-user (HKCU) environment here, not from a [Registry] }
{ entry: an entry with a ValueName but no ValueData creates the key and writes   }
{ no value, so the two would be a silent second route through the same registry  }
{ value. The install is per-user (LocalAppData, no admin), so we must target      }
{ HKCU rather than HKLM.                                                         }

const
  HWND_BROADCAST = $FFFF;
  WM_SETTINGCHANGE = $001A;
  SMTO_ABORTIFHUNG = $0002;

{ Widely-used recipe for calling the Unicode user32 API from [Code]: the }
{ PChar parameter arrives as a pointer to the wide characters of         }
{ 'Environment', which is exactly the LPARAM the message wants.          }
function SendMessageTimeoutW(hWnd: LongWord; Msg: LongWord; wParam: LongWord;
  lParam: PChar; fuFlags: LongWord; uTimeout: LongWord;
  var lpdwResult: LongWord): LongWord;
  external 'SendMessageTimeoutW@user32.dll stdcall';

{ Broadcast WM_SETTINGCHANGE ("Environment" changed) so Explorer and newly }
{ started processes reload the PATH without a re-login. }
procedure BroadcastEnvironmentChange;
var
  DummyResult: LongWord;
begin
  SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, 'Environment',
    SMTO_ABORTIFHUNG, 5000, DummyResult);
end;

{ True when Entry is already one of Path's `;`-separated entries. A plain }
{ Pos() substring test is not enough: an existing `C:\Users\a\x-tools`    }
{ would suppress adding `C:\Users\a\x`. Entries are compared whole,       }
{ trimmed, and case-insensitively (Windows path matching ignores case).   }
function PathHasEntry(Path: string; Entry: string): Boolean;
var
  Rest: string;
  Item: string;
  Semi: Integer;
begin
  Result := False;
  Entry := Trim(Entry);
  Rest := Path + ';';
  while Pos(';', Rest) > 0 do
  begin
    Semi := Pos(';', Rest);
    Item := Trim(Copy(Rest, 1, Semi - 1));
    if (Item <> '') and (Lowercase(Item) = Lowercase(Entry)) then
    begin
      Result := True;
      exit;
    end;
    Delete(Rest, 1, Semi);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  OrigPath: string;
  NewPath: string;
  AppDir: string;
begin
  if (CurStep = ssPostInstall) and IsTaskSelected('path') then
  begin
    if RegQueryStringValue(HKCU, 'Environment', 'Path', OrigPath) then
    begin
      AppDir := ExpandConstant('{app}');
      NewPath := OrigPath;
      if not PathHasEntry(NewPath, AppDir) then
      begin
        if (Length(NewPath) > 0) and (Copy(NewPath, Length(NewPath), 1) <> ';') then
          NewPath := NewPath + ';';
        NewPath := NewPath + AppDir;
        RegWriteStringValue(HKCU, 'Environment', 'Path', NewPath);
        BroadcastEnvironmentChange;
      end;
    end;
  end;
end;
