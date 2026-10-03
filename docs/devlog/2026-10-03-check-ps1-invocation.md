# 2026-10-03 · `scripts/check.ps1` 的跑法，和一个会报绿的闸门

现象：Git Bash 里 `./scripts/check.ps1` 不是「找不到解释器」，而是 bash 把这个
PowerShell 文件当 shell 脚本读，报出

```
./scripts/check.ps1: line 5: =: command not found
./scripts/check.ps1: line 8: syntax error near unexpected token `[string]$Name,'
```

排查中测出三条 PowerShell / msys 的隐藏行为，都不是读那 24 行脚本能看出来的。

## 1. shebang 能让 Git Bash 跑 .ps1，但必须写 `-File`

加 `#!/usr/bin/env -S powershell -NoProfile -ExecutionPolicy Bypass -File` 之后
`./scripts/check.ps1` 交给 `powershell.exe`；`-S` 是给 shebang 带多个参数用的
（msys2 coreutils 支持），这一行在 PowerShell 侧只是普通注释。

**退出码差异是实测出来的**：脚本 `exit 3` 时

| 调用 | bash 看到 |
| --- | --- |
| `powershell.exe ./t.ps1` | 1 |
| `powershell.exe -File ./t.ps1` | 3 |
| 脚本 `exit 0`，两种写法 | 0 |

也就是说少了 `-File`，回来的数不是检查产生的数（红还是红，但码被压平）。这个脚本
存在的意义就是当闸门，所以 shebang 里 `-File` 不是风格问题。

## 2. `$LASTEXITCODE` 会跨步骤残留，能把没跑起来的步骤读成通过

`Invoke-Step` 原来只判断 `$LASTEXITCODE -ne 0`。把中间一步换成不存在的命令，
PowerShell 抛 `CommandNotFoundException`，但**不产生新的退出码**——于是这一步读到
的是上一步留下的 `0`，闸门打印 `==> Clippy` 之后继续跑，整跑仍然 `all checks passed`
并退出 0。cargo 不在 PATH 时就是这个形状：三个步骤全部「通过」。

顺手测了两种信号，哪个能用哪个不能用：

- 调用方 `& $Body` 之后的 `$?` 是 **True**，抓不到（错误发生在 scriptblock 里，
  `$?` 反映的是这次调用本身）。
- `$Error.Count` 会 +1；而 cargo 全部进度都往 stderr 写，`$Error.Count` **不变**
  （native 命令失败也不增长，只反映在 `$LASTEXITCODE`）。所以「数错误记录」
  不会把绿跑判成红。

处理：每步执行前 `$global:LASTEXITCODE = $null` 清一次，之后「根本没有退出码」
就是这一步自己的事实，而不是从别处继承的数；失败信息也分成两种——真实的
`exit N`，或者 `no command ran; is cargo on PATH?`。上一条里那个「FAILED (0)」
这种看着像反话的输出，就是先改了一版但没清码留下的。

## 验证

- 合成闸门（三步分别换成 `cmd.exe /c exit 0` / 不存在的命令 / `exit 7`）：只有
  不存在命令那步报 `FAILED (no command ran; is cargo on PATH?)`，整体退出 1。
- 三步全绿且每步都往 stderr 写噪音：`all checks passed`、退出 0。
- 真跑三条路径都是 EXIT=0：Git Bash `./scripts/check.ps1`、
  `powershell -NoProfile -File scripts/check.ps1`、`cd scripts && ./check.ps1`
  （最后一条能过是因为 cargo 会自己往上找 workspace 根）。
- 未覆盖：本机没装 `pwsh`（PowerShell 7），shebang 指向 `powershell`（5.1，每台
  Windows 都在）；Restricted 执行策略下直接 `.\scripts\check.ps1` 没测，本机是
  RemoteSigned 且文件是本地文件，不需要 Bypass。
