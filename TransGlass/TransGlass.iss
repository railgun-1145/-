; TransGlass Inno Setup Script
; 构建可分发安装包（含卸载、开机自启可选）

#define MyAppName "TransGlass"
#define MyAppPublisher "railgun-1145"
#define MyAppURL "https://github.com/railgun-1145/TransGlass"
#define MyAppExeName "transglass.exe"

; --- 路径配置 ---
; 如果未通过编译器命令行 (/DMyAppSourceRoot=...) 指定路径，则使用默认相对路径
#ifndef MyAppSourceRoot
  #define MyAppSourceRoot "."
#endif

#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

[Setup]
; 注: AppId 应保持唯一以识别程序
AppId={{C1E0B6B4-18A0-4D5D-9D7A-2C5DFD4D0A2F}}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
; 只有安装包具有数字签名时，以下两项才有意义
; SignedUninstaller=yes
; SignTool=...
DisableDirPage=no
DisableProgramGroupPage=no
ShowLanguageDialog=no
OutputBaseFilename=TransGlass_{#MyAppVersion}_Setup
OutputDir={#MyAppSourceRoot}\dist\installer
Compression=lzma2
SolidCompression=yes
DefaultDirName={localappdata}\Programs\{#MyAppName}
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
UninstallDisplayIcon={app}\{#MyAppExeName}

[Languages]
Name: "schinese"; MessagesFile: "{#MyAppSourceRoot}\\installer\\Languages\\ChineseSimplified.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "startup"; Description: "开机自启 (当前用户)"; GroupDescription: "附加任务"; Flags: unchecked

[Files]
Source: "{#MyAppSourceRoot}\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#MyAppSourceRoot}\transglass.manifest"; DestDir: "{app}"; Flags: ignoreversion
; 包含默认配置文件（如果存在）
Source: "{#MyAppSourceRoot}\transglass_hotkeys.json"; DestDir: "{app}"; Flags: ignoreversion onlyifdoesntexist skipifsourcedoesntexist

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#MyAppName}"; ValueData: """{app}\{#MyAppExeName}"""; Tasks: startup; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: postinstall nowait skipifsilent

[UninstallDelete]
; 卸载时清理生成的配置文件
Type: files; Name: "{app}\transglass_hotkeys.json"
Type: files; Name: "{app}\transglass.manifest"
Type: dirifempty; Name: "{app}"

[Code]
// 这里可以添加更复杂的 Pascal 脚本，例如检查进程是否正在运行
function InitializeSetup(): Boolean;
var
  ErrorCode: Integer;
begin
  Result := True;
  // 可以在此处添加安装前的环境检查
end;
