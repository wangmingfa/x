# 2026-10-01 · P2 续：防火墙落地 + 三平台编译/lint 修复

P2 的前七个域（proxy / hosts / power+time / mount / permission / startup /
schedule）已经写完但只有模型和适配器；本次把第八个域防火墙补完（CLI、审计、
测试、文档），并把整套 P2 代码推到「三平台全绿」：本机 `scripts/check.sh`、
`cargo clippy --target x86_64-unknown-linux-gnu`、`--target
aarch64-apple-darwin` 全部 `-D warnings` 通过。

## 防火墙

- **netsh 在本机（zh-CN Windows 11）输出的是 UTF-8，不是 OEM 代码页**，且
  键名全本地化：`状态 / 规则名称 / 操作 / 本地端口 / 协议 / 已启用 /
  任何 / 阻止 / 允许 / 是 / 否`。`enabled()` 最初照抄英文
  `contains("ON)")` 的写法永远匹配不上；现在按「行以 状态/State 开头且含
  ON/启用」判定，规则解析（`parse_windows_rules`）双语键名一起收。
  复用 `windows::service::decode_console`（先 UTF-8、失败再
  `MultiByteToWideChar(CP_OEMCP)`），它因此从私有升为 `pub(crate)`。
- **Windows 的 deny 语义 = 加一条 Block 规则**：Windows 防火墙里阻断优先于
  放行，不是"删除 Allow"。默认规则名取 `x-{allow|deny}-{port}-{proto}`，
  避免同名规则相互覆盖。
- **ufw `status numbered` 的行位移**：`[ 1] 22/tcp  ALLOW IN  Anywhere` 前两
  个 token 是序号，直接按"第一列是端口"解析会全军覆没；叙述体
  `Anywhere on 53/tcp` 还要倒过来取端口。端口区间里的 `-` `:` 校验必须用
  字节字面量（迭代 `.bytes()` 时 `b == '-'` 是 u8 对 char，Linux 目标编译
  直接报错，见下）。
- **Linux 的 `--name` 不静默丢弃**：ufw 支持 `comment <文本>` 子命令，规则名
  落成 ufw 注释；macOS 的 pf 改规则需要锚点+root，`allow`/`deny` 如实报
  Unsupported，而不是拿 socketfilterfw 假装改成了。
- 审计沿用 P0 的装饰器约定：`AuditedFirewall` 只记 `allow`/`deny`
  （失败与被拒也记），`status`/`list` 是读操作不留痕；target 形如
  `port 53/udp as \`dns\``。挂载点仍只有组合根 `audit::attach`。
- CLI 确认被拒走 `Error::invalid_input`（退出 5，"aborted by user"），与
  hosts/mount/startup 这一批新命令一致；port/ps kill 的 130 是历史语义，
  两者并存。`--json` 跳过确认提示。

## 三平台 lint / 编译坑（本次最大的收获）

- **clippy 1.98 的 `needless_return` 会按"当前 OS 展开 cfg 后"生效**：
  形如 `#[cfg(windows)] { … return X; }` 后面跟着 cfg-off 分支的写法，在
  Windows 上 X 成了函数尾表达式 → `-D warnings` 炸。修法是每个平台分支
  改成 `{ 尾表达式 }`，**但裸表达式行 `#[cfg(x)] f()`（无分号）后还有其它
  cfg 项时连解析都过不了——只有块形式合法**。
- **单平台跑 clippy 看不见别的 OS 的坑**：本机修复只覆盖 Windows 分支。
  本地近似 Linux / macOS runner：
  `cargo clippy -p x-platform --target x86_64-unknown-linux-gnu --all-targets -- -D warnings`
  （`aarch64-apple-darwin` 同理；元数据编译，不需要链接器）。这次揪出：
  - `shell_os`：`Error` 没在 `#[cfg(not(windows))]` 下导入；
    `getent passwd` 把 `Option<String>` 直接喂给 `.arg()`；
  - `user_os::current()`：unix 分支给 `info(&str)` 传了 `&Option<String>`；
  - `startup_os`：`PathBuf` 只在 macOS 导入，但 Linux 的
    `autostart_dir()` 也返回它；macOS 不 spawn，`Command` 导入要
    `#[cfg(not(target_os = "macos"))]`；
  - `schedule_os`：macOS 的 `value.parse::<u64>().filter(...)`——`Result`
    没有 `filter`，要先 `.ok()`；`crontab` 无条目名，Linux 分支 `name`
    未用需 `let _ = name;` 写明意图；
  - `macos/disk.rs`：`return Some(rest…next()?)` 触发
    `needless_question_mark`，Option 尾值直接返回即可。
- **x-core 架构守卫是字符串扫描，连 `cfg!(...)` 宏形态也算违规**。P2 有两处
  撞线：`hostsfile::default_path` 里的 `cfg!(target_family = "windows")` 和
  `pathperm::has_executable_bit` 里的 `#[cfg(unix)]`。修复没有放宽守卫而是
  把 OS 知识挪层：hosts 路径进 `x_platform::common::hosts_os::default_path`；
  `pathperm::check(path, is_executable: fn(&Path) -> bool)` 改为由调用方传入
  `x_platform::common::pathperm_os::has_executable_bit`——x-core 测试也随之
  能注入假探针，不再依赖本机 OS。
- `OpenOptions` 同时给 `.write(true).append(true)` 会被
  `ineffective_open_options` 判违规：`append` 已蕴含写，留一个就够。

## 本机活体验证

- `x firewall status` → `stack windows_firewall / enabled yes`；
  `--json` 给 `{"enabled":true,"stack":"windows_firewall"}`。
- `x firewall list --limit 8` → 真实规则名、端口、协议、✓；尾行
  `… 490 more rule(s) (use --limit)`。
- `echo n | x firewall allow 8080` → `about to open tcp port 8080` +
  `error: aborted by user`，退出 5，**netsh 没被调到、规则没动**（真实
  allow/deny 会改系统防火墙，验证前需用户同意，本轮没有执行）。
- 重构回归：`x hosts path` → `C:\WINDOWS\System32\drivers\etc\hosts`；
  `x permission check execute x.exe` → allowed（退出 0）。
