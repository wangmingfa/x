# 2026-10-03 — installer 编译失败：AnsiLastChar 与两处恒真守卫

## 现象

`release.yml` 的 windows-installer 作业在 `Compile installer` 步骤失败：

```
Compiling [Code] section
Error on line 96 in D:\a\x\x\Packaging\Windows\x.iss: Column 39:
Unknown identifier 'AnsiLastChar'
Compile aborted.
Error: Process completed with exit code 1.
```

编译器是 runner 自带的 Inno Setup 6.7.1。`AnsiLastChar` 是 Delphi `Windows` 单元的函数，
Inno Setup 的 Pascal Script 不带它——官方 support-functions 参考里查不到，
而 `Copy` / `Length` / `Pos` 都在。改成 `Copy(NewPath, Length(NewPath), 1) <> ';'`。

结构性原因：**`ci.yml` 里没有任何步骤编译 `x.iss`**（`grep -i 'iscc|installer|\.iss'` 无匹配）。
这个文件从写下来到这次才第一次被编译器读，所以「只在 release 时炸」不是偶发。

## 日志能证明什么，不能证明什么

ISCC 是顺序编译、遇到第一个错误就 abort。报错停在 96 行，说明它前面的
`MsgResult: DWORD`（87 行）和 `IsTaskSelected`（89 行）都通过了编译器——这两个标识符合法。
100 行的 `SendMessageTimeout(HWND_BROADCAST, WM_SETTINGCHANGE, 0, LPARAM('Environment'),
SMTO_ABORTIFHUNG, 5000, MsgResult)` 排在 96 之后，**从来没被编译过**，日志对它一个字没说。
官方参考里查不到 `SendMessageTimeout` / `HWND_BROADCAST` / `WM_SETTINGCHANGE` /
`SMTO_ABORTIFHUNG`。而 `[Setup]` 第 42 行已经写了 `ChangesEnvironment=yes`，
这个 directive 的用途就是装完后通知运行中的应用「环境变量变了」——手写的广播是重复的，
而且是一个我无法本地证实的调用。删掉它，同时删掉 `MsgResult`。

## 顺带查出的两处恒真

都在 PATH 那一段，都是「守卫看着对、实际永远为真」：

- `[Registry]` 条目 `Root: HKLM; … ValueName: "Path"; Tasks: path; Check: NeedsAddPath('{app}')`
  没有 `ValueData`。按官方 `[Registry]` 文档，省掉 `ValueData` 就不写任何值——它只打开/创建键，
  PATH 从来没被这条改过。而它上面的注释写着「Append the install dir to the system PATH」，
  是假的。删掉条目和只服务于它的 `NeedsAddPath`。
- `Check` 的字符串参数**不会**自动做常量展开（官方 Check 文档），所以 `NeedsAddPath` 拿到的
  `Param` 是字面量 `{app}`，永远匹配不上注册表里的 Path → 守卫恒真。同样的坑在
  `CurStepChanged` 里也有一份：`Pos('{app}', NewPath)`——`[Code]` 里必须 `ExpandConstant`
  才展开。也就是说覆盖安装一次，PATH 就多一条重复项。改成先 `AppDir := ExpandConstant('{app}')`，
  再拿 `AppDir` 判断和写入。

现在 PATH 只有一个写入点（`[Code]` 读-改-写），注释与代码一致。

## 已验证

- `bash scripts/check.sh` EXIT=0（Rust 侧未受影响，改动不在编译单元里；跑闸门是提交前的固定动作）。

## 未验证 —— 说清楚

- **本机没有 Inno Setup**（`C:\Program Files (x86)\Inno Setup 6\ISCC.exe` 不存在），
  所以这次改动没有经过编译器第二遍。`AnsiLastChar → Copy(...)` 是文档核对的结果。
- PATH 追加的实际行为要真装一次安装包才知道，CI 只编译不安装。
- 删掉 `SendMessageTimeout` 后，通知依赖 `ChangesEnvironment=yes`；这一点同样只对着文档读的。

残余风险不为零：如果 `[Code]` 里还有别的未支持标识符，下一次 compile 会报在别的行。

## 没做，留给你选

- 把「编译安装包」挪进 `ci.yml` 的 windows 作业，让每步 push 都过一遍 ISCC。
  代价是要有个 exe 给 `[Files]` 的 `Source` 用；`x.iss` 里 `#define SourcePath` 现在是写死的，
  把它包一层 `#ifndef SourcePath` 就能让 CI 传 `/DSourcePath=..\..\target\debug\x.exe`，
  不必额外跑 `--release`。
- 或者本机装 portable 版 Inno Setup，那样这类错误我在交给你之前就能自己验。
- 卸载后 PATH 里残留 `{app}`：`[Code]` 写的值 Inno 不会在卸载时还原。
  要用官方 `{olddata}` 那条路由才谈得上还原，而那需要真装一遍来验证。
