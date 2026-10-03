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

## 闸门修好后立刻抓到的第一个失败：两个只在 PowerShell 下红的测试

`.\scripts\check.ps1` 在 PowerShell 里报

```
common::process_sysinfo::tests::kills_a_process_it_started
  panicked at crates\x-platform\src\common\process_sysinfo.rs:511: spawn sleep: Error { kind: NotFound, message: "program not found" }
```

`crates/x-platform/src/common/process_sysinfo.rs` 的两个进程测试 spawn 的是
`sleep` 和 `true`——POSIX core。它们在 `D:\Apps\Git\usr\bin` 里，而这一目录只在
**Git Bash 的 PATH** 上；PowerShell 的 PATH 没有，于是 `Command::new("sleep")`
直接 NotFound。这两个测试没有 `#[cfg(unix)]`（对比 `common/identity.rs:302` 那个用
`date` 的测试就正确地关在了 unix 里），所以它们测的是 Windows 也要跑的 kill 路径。

**为什么本地一直没发现**：`bash scripts/check.sh` 起的是 bash，`cargo test` 继承了
Git Bash 的 PATH——同一台机器、同一份代码，换个 shell 跑就是两种结果。CI 的
windows-latest 也是绿的：那个作业没有写 `shell:`（用的默认 pwsh），测试却通过，
说明 runner 镜像的 PATH 里带着 Git 的 `usr\bin`（这一条是从「CI 绿」反推的，没在
runner 上直接验过）。也就是说这个测试从来没有在「用户真实的 Windows 环境」里跑过。

处理：测试自己按平台挑程序（`stays_alive()` 用 `ping -n 30 127.0.0.1`，
`exits_immediately()` 用 `cmd /c exit 0`；非 Windows 仍是 `sleep` / `true`），
不再把「哪个 shell 启动了 cargo」当成测试前提。

## 验证

- 合成闸门（三步分别换成 `cmd.exe /c exit 0` / 不存在的命令 / `exit 7`）：只有
  不存在命令那步报 `FAILED (no command ran; is cargo on PATH?)`，整体退出 1。
- 三步全绿且每步都往 stderr 写噪音：`all checks passed`、退出 0。
- 上面两条只验证了闸门本身。要验证那两个测试，得先把 PATH 换成 PowerShell
  用户实际拿到的那一份：`$env:PATH` 只留
  `C:\Windows\System32;C:\Windows;C:\Windows\System32\Wbem` + cargo + `git\cmd`，
  实测 `sleep.exe` / `true.exe` 都不在其中（`ping.exe`、`cmd.exe` 在）。这个环境里
  `cargo test --workspace` 退出 0，`kills_a_process_it_started` 与
  `killing_a_dead_pid_reports_not_found` 都是 `ok`，x-platform 99 passed / 0 failed；
  再跑 `& .\scripts\check.ps1` 同样三步全绿、退出 0。
- **不作数的一版验证**：改完先在 Git Bash 里 `powershell -File scripts/check.ps1`
  跑通过——但那个 `powershell.exe` 是 bash 起的，继承的还是 Git Bash 的 PATH，
  对 PATH 敏感的测试在这种「换了 shell、没换 PATH」的跑法里永远是绿的。判断
  PATH 相关的失败，必须以显式赋值后的那一次为准。
- 闸门的三条调用路径都是 EXIT=0：Git Bash `./scripts/check.ps1`、
  `powershell -NoProfile -File scripts/check.ps1`、`cd scripts && ./check.ps1`
  （最后一条能过是因为 cargo 会自己往上找 workspace 根）。这一组只证明「怎么叫都能
  跑、回来的码是检查自己的」；它对 PATH 依赖不作数——那三条全是从 Git Bash 起的。
- 未覆盖：本机没装 `pwsh`（PowerShell 7），shebang 指向 `powershell`（5.1，每台
  Windows 都在）；Restricted 执行策略下直接 `.\scripts\check.ps1` 没测，本机是
  RemoteSigned 且文件是本地文件，不需要 Bypass。
