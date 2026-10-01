# x

在一套一致的词汇下查看进程、端口、网络、服务、磁盘和系统健康状态，
跨 Windows / Linux / macOS，既可脚本化，也可交互使用。

- `x port`、`x ps`、`x sys`、`x net`、`x disk`、`x service` —— 一次性命令，
  支持 JSON / 制表符 / 表格三种输出，退出码稳定。
- `x tui`（或 `x ui`）—— 基于同一份数据的交互式仪表盘。

适配器原生优先：能用 Rust 库和系统 API 就用，否则回退到公认的命令。

## 构建

```sh
cargo build -p x-app
```

产物为 `target/debug/x`（或 `release/x`）。

## 当前功能

### 端口（`x port`）

| 命令 | 说明 |
| --- | --- |
| `x port` | 监听中的 socket 及其属主进程 |
| `x port all` | 全部 socket（不只监听），含远程地址列 |
| `x port owners` | 按进程分组 |
| `x port stats` | 按状态 / 协议统计 socket 数，并给出内核队列占用（平台提供时） |
| `x port find <name>` | 按进程名反查其持有的 socket |
| `x port watch` | 持续轮询，只在 socket 增删时输出（`--interval`、`--count`） |
| `x port check <port>` | 占用返回 0，空闲返回 1 |
| `x port kill <port>` / `kill-by-name <name>` | 先展示计划，确认后停止持有端口的进程（`--yes` 跳过确认） |

### 进程（`x ps`）

| 命令 | 说明 |
| --- | --- |
| `x ps [搜索词]` | 进程列表，默认按 CPU 排序；支持 `--sort`（cpu / memory / pid / name / start-time）、`--limit`、`--user`、`--light`（跳过 CPU/内存采样） |
| `x ps tree` | 进程树 |
| `x ps show <pid>` | 单个进程详情 |
| `x ps kill <pid>` / `kill-by-name <name>` | 确认后发信号，`--signal TERM / INT / HUP` |

### 系统（`x sys`）

| 命令 | 说明 |
| --- | --- |
| `x sys info` | 操作系统、内核、架构、主机名、CPU 型号、核数（含 P/E 核心划分）、内存、运行时长、上次重启、时区、locale、用户 / shell / 终端 |
| `x sys cpu` | 聚合与每核 CPU 利用率，负载均值、当前/最大频率（平台提供时）、温度（Linux）、governor（Linux） |
| `x sys mem` | 内存利用率 |

### 网络（`x net`）

| 命令 | 说明 |
| --- | --- |
| `x net interfaces` | 网卡：状态、MAC、MTU、链路速度、收发流量、默认网关、默认路由标记、链路类型 |
| `x net addresses` | IP 地址 + 前缀长度（来自 netmask）+ DHCP 标记 |
| `x net routes` | 路由表（IPv4/IPv6） |
| `x net dns` | DNS 解析器配置 |
| `x net flush` | 刷新系统 DNS 缓存 |
| `x net connections` | 全部套接字 + 归属进程，`--process` / `--port` 过滤 |
| `x net resolve <host>` | 正向解析：主机名 → 地址 |
| `x net reverse <ip>` | 反向解析：地址 → 主机名 |
| `x net ping <host>` | ICMP 连通性（`--count` / `--timeout`），输出 min/avg/max |
| `x net trace <host>` | 逐跳路径（`--max-hops` / `--timeout`），原生 ICMP，不依赖 tracert |
| `x net check <host>` | 链式诊断：DNS → TCP → TLS → 证书 → HTTP，任一环失败如实标注 |

### 磁盘（`x disk`）

| 命令 | 说明 |
| --- | --- |
| `x disk list` | 全部挂载文件系统：设备、类型、介质、容量、已用、可用、使用率（`--json` 含标签 / 卷 UUID / 分区 UUID / 型号 / 序列号 / 只读） |
| `x disk current` | 当前目录所在的文件系统 |
| `x disk usage <path> [--depth N]` | 目录占用，du 风格：按聚合大小排序，`--depth` 限制展示层级，路径不存在退出码 3 |

### 服务（`x service`）

| 命令 | 说明 |
| --- | --- |
| `x service list` | 服务列表及状态 |
| `x service status <name>` | 单个服务详情 |
| `x service start / stop / restart / reload / enable / disable <name>` | 确认后对单个服务执行操作 |
| `x service logs <name> [--lines N]` | 服务最近的日志（journalctl / log show / 事件日志），最新在前 |
| `x service native <args>…` | 逃生舱：参数原样交给平台管理器命令（systemctl / launchctl / sc），子进程非零退出则 x 退出码 1 |

### 防火墙（`x firewall`）

| 命令 | 说明 |
| --- | --- |
| `x firewall status` | 哪个防火墙在应答（Windows Firewall / netfilter / pf）与是否启用 |
| `x firewall list [--limit N]` | 可见规则；截断时说明还有多少条 |
| `x firewall allow <port> [--proto tcp\|udp] [--name N]` | 放行入站端口（需管理员 / root，先确认） |
| `x firewall deny <port> [--proto tcp\|udp] [--name N]` | 阻断入站端口（Windows 上落为 Block 规则，阻断优先于放行） |

读取走各平台自己的工具（`netsh advfirewall` / `ufw`、`nft`、`iptables` /
`socketfilterfw`）；macOS 的 pf 规则修改需要锚点与 root，`allow` / `deny`
如实报不支持。`allow` / `deny` 与所有破坏性操作一样先确认、被拒留痕
（审计动作 `firewall.allow` / `firewall.deny`）。

### 系统日志（`x logs`）

| 命令 | 说明 |
| --- | --- |
| `x logs system [--limit N]` | 整机日志：journald / System 通道 / unified log |
| `x logs service <name> [--limit N]` | 按服务取：systemd unit / 事件提供方 / 守护进程名 |
| `x logs process <pid\|name> [--limit N]` | 按进程取（Windows 事件日志无进程索引，如实报不支持） |

全部只读：不确认、不提权、不落审计。三套日志体系词汇不通，故时间戳与
级别按平台原文透传（如 journald 的 `err`、中文 Windows 的「信息」），
数据源为各平台原生工具的结构化输出（`journalctl -o json` /
`Get-WinEvent` + `ConvertTo-Json` / `log show --style json`），不做文本
刮取；新→旧排序，默认 50 条。

### 设备清单（`x device`）

| 命令 | 说明 |
| --- | --- |
| `x device list [--class usb]` | 全部在场设备，可按类别过滤 |
| `x device usb / bluetooth / audio / display / camera / input / network` | 对应类别的快捷方式 |
| `x device list --json` | 整份清单的 JSON 数组 |

数据源为平台原生枚举：Windows `Get-PnpDevice -PresentOnly`（JSON 投影）、
Linux sysfs（`/sys/bus/usb/devices`、`/sys/class/*`、`/proc/asound/cards`）、
macOS `system_profiler -json`。平台自己的类别/状态原文保留在
`class_raw` / `status`（中文 Windows 的「蓝牙枚举器」原样透传），归一化
类别只归明确认识的 token，归不了的进 `other`，不做猜测；字段没报就整列
缺席。全部只读，不落审计。

### 蓝牙（`x bluetooth`）

| 命令 | 说明 |
| --- | --- |
| `x bluetooth adapters` | 本机蓝牙控制器 |
| `x bluetooth devices` | 平台已知设备（已配对 / 已枚举） |
| `x bluetooth scan [--timeout 10]` | 定时发现附近设备（Linux） |
| `x bluetooth connect <MAC> [-y]` | 连接设备（Linux） |
| `x bluetooth disconnect <MAC> [-y]` | 断开连接（Linux） |

三平台能力天然不对等，如实呈现：Linux 用 BlueZ 自带的 `bluetoothctl`
（`list`/`show`/`devices`/`--timeout N scan on`/`connect`/`disconnect`，
全套可用）；Windows 读 `Get-PnpDevice -Class Bluetooth` 的 JSON 投影，
macOS 读 `system_profiler SPBluetoothDataType -json`——两者没有命令行
连接/断开的第一方通道，动词如实返回不支持（退出码 7），不代 GUI 操作。
地址在入口统一校验并规范成大写冒号分组的 `AA:BB:…`，坏输入先拒后审。
`connect` / `disconnect` 与 firewall 同规格：先确认、经审计装饰器落盘
（被平台拒绝的尝试同样留痕）；`adapters` / `devices` / `scan` 是读，
不留痕。平台没报 `powered` / `paired` 等标志就显示 `-`，JSON 里整字段
缺席，不拿「未知」冒充「否」。

### 显示与显示器（`x display`）

| 命令 | 说明 |
| --- | --- |
| `x display list` | 拓扑表：连接、分辨率、刷新率、缩放、主屏、位置 |
| `x display info <序号或名称>` | 单台显示器的全部字段（名称模糊匹配，歧义即拒） |
| `x display list --json` | 整份拓扑的 JSON 数组（没报的字段整列缺席） |

Windows 走 Gdi32 原生枚举（`EnumDisplayDevicesW` 逐监视器、`EnumDisplay
SettingsW(ENUM_CURRENT_SETTINGS)` 取当前模式与桌面位置、`GetDpiForMonitor`
取有效缩放）——`Win32_VideoController` 这类投影无法与监视器一一对应，
不做拼图式猜测；Linux 读 DRM 连接器的 sysfs（`status` 判连接、
`modes` 首行即当前模式 `1920x1080 60.00`），位置/主屏/缩放属合成器状态，
如实缺席；macOS 解析 `system_profiler SPDisplaysDataType -json`，含
Retina「as W x H」缩放换算。全部只读，不落审计。

### 窗口（`x window`）

| 命令 | 说明 |
| --- | --- |
| `x window list` | 打开窗口清单：标题、应用、PID、激活 / 最小化 / 最大化 |
| `x window active` | 当前前台窗口（没有前台窗口时如实说无） |
| `x window focus <序号或标题> [-y]` | 把窗口带到前台 |
| `x window minimize <序号或标题> [-y]` | 最小化窗口 |
| `x window maximize <序号或标题> [-y]` | 最大化窗口 |

Windows 走 user32 原生（`EnumWindows` + `GetWindowTextW` 列窗口，
`IsIconic` / `IsZoomed` 判状态，`SetForegroundWindow` / `ShowWindow`
执行动词）；前台锁拒绝置顶时如实报 InvalidState 并说明原因，不假装
成功。Linux 用 `wmctrl` 列窗口、`xprop` 读 `_NET_ACTIVE_WINDOW` /
`_NET_WM_STATE`，动词投递 `wmctrl -i -a` 与 `-b add,hidden` /
`add,maximized_*`；两者缺一或 `DISPLAY` 未设即如实不支持（附安装
提示）。macOS 用 `CGWindowListCopyWindowInfo` 免权限列出窗口——未授
「屏幕录制」时标题整列缺席；动词与 active 走 `osascript` System Events，
未授「辅助功能」的拒绝原样透出（错误码 -1719 / -25211）。

窗口 id 统一规范成 `0x{:08X}`（wmctrl 补零与 xprop 无补零因此可比）；
目标按 1 起序号或标题选择，歧义即拒。`focus` / `minimize` / `maximize`
与 firewall 同规格：先确认、经审计装饰器落盘（平台拒绝同样留痕）；
`list` / `active` 是读，不留痕。平台没报的状态显示 `-`，JSON 里整字段
缺席，不拿「未知」冒充「否」。

### 事件（`x events`）

| 命令 | 说明 |
| --- | --- |
| `x events [--interval 2.0] [--count N] [--types LIST]` | 轮询采样五个家族并只打印两轮之间的差异：进程启停、连接开闭、USB 插拔、挂载增删、服务出现 / 消失 / 状态迁移 |

`--types` 取值 `process,connection,usb,mount,service,all`；缺省是除
usb 外的全部（`all` 显式包含 usb）。usb 缺省关掉不是偷懒：Windows
侧采样要 shell 出 `Get-PnpDevice`，实测每次约 1.6 秒，轮询太重，
要盯 USB 就显式写 `--types usb` 或 `all`。

事件不是内核订阅，而是「快照对拍」：x-core 每次轮询调各管理器取一份
快照，和上一轮做差。行前缀 `+` 出现 / `-` 消失 / `~` 状态迁移；某一家
族本轮读失败（如连接表权限不足）时如实打 `! 家族 采样失败: 原因`，
该家族这一轮不参与对拍，恢复后以新一轮为基线，绝不把「读不到」渲染成
「全没了」的假事件潮。JSON 模式每轮一份文档，且只在有差异或失败时
输出。Ctrl-C 前持续轮询，`--count` 限轮数。只读，不落审计。

### 证书与网络诊断（`x cert` / `x tls` / `x http` / `x net check`）

| 命令 | 说明 |
| --- | --- |
| `x cert check <host> [--port 443] [--timeout N]` | TLS 证书：协议、加密套件、Subject/Issuer、有效期、SAN、链校验结果 |
| `x tls <host> [--port] [--timeout]` | TLS 会话概况（协议 / 套件 / 信任），不展开证书字段 |
| `x http <url> [--method GET] [--timeout N]` | HTTP 探测：状态码、版本、总耗时、远端 IP、字节数 |
| `x headers <url> [--method] [--timeout]` | 响应头逐行展示 + 状态与耗时 |
| `x net check <host> [--port] [--timeout]` | DNS → TCP → TLS → 证书 → HTTP 逐环诊断，✓/!/✗ 三态，后环拿不到前环结果时如实标 skipped |

不自带 TLS/HTTP 栈：探测交给平台工具（curl 三平台、Windows 用
PowerShell SslStream、类 Unix 用 openssl s_client），x 负责参数校验、
输出结构化与退出码归因（如 curl 超时 → 6、拒绝连接 → 3）。全部是网络
读操作，不写审计。链式诊断取 `x net check` 而非路线图里的
`x doctor <host>`，因为 `x doctor` 已用于开发者体检。

### 能力探测（`x capability`）

| 命令 | 说明 |
| --- | --- |
| `x capability` | 逐特性实测本机：supported / degraded / unsupported，附原因 |
| `x capability --domain <system\|process\|port\|net\|disk\|service>` | 只看一个域 |

探测只跑读操作与安全的环回操作（ping/trace 打向 127.0.0.1）；kill、服务
动作等破坏性项只报告"已实现、未实测、受权限约束"，不会在探测中执行。

### 审计日志

破坏性操作在真实运行时（CLI 与 TUI 共用同一条路径）逐条追加写本地日志，
一行一个 JSON（UTC 时间、用户、动作、目标、结果），被拒绝的操作同样留痕：

| 动作 | 记录内容 |
| --- | --- |
| `process.kill` / `process.kill_many` | pid 与信号；批量 kill 记一条 |
| `port.kill_plan` | 查询目标（`port 8080` / `process node`）、杀掉进程数 |
| `service.action` | 服务名与动词（start / stop / …） |
| `service.native` | 原生命令完整参数与退出码（`x capability` 的空参数探测不记录） |
| `firewall.allow` / `firewall.deny` | 端口、协议与规则名（如 `port 8080/tcp as "web"`） |

读操作不留痕：`x logs` 读系统日志、`x device` 枚举设备本身都不写审计行，
审计只覆盖改变机器的动作。

日志位置：Windows `%LOCALAPPDATA%\x\audit.log`；Linux
`$XDG_STATE_HOME`（缺省 `~/.local/state`）`/x/audit.log`；macOS
`~/Library/Application Support/x/audit.log`。环境变量 `X_AUDIT_PATH`
指定完整路径，`X_AUDIT=off` 关闭审计。写入失败绝不影响被审计的操作。

### 交互式 TUI（`x tui` / `x ui`）

七个页面：Dashboard（首页）、Ports、Processes、Network、Services、
System、Disks；侧边栏列页，数字 `1`–`7` 直达，`Tab`/左右键循环。
可见页每 1.5 秒自动刷新，失败保留旧数据、错误进状态栏。

- 首页：CPU / 内存仪表、端口计数（监听 / 已建立 / 全部）、系统事实与
  文件系统表
- 命令面板：`Ctrl+P` 打开，输入即过滤命令，`Enter` 执行（覆盖翻页、
  刷新、过滤、搜索、kill、树形、排序、跳到选中进程的端口等）
- 全局搜索：`/` 同时命中进程 / 端口 / 服务 / 网络 / 文件，回车跳到
  对应页面并带上过滤；某家族采样失败会在结果里就地注明
- 进程页：`t` 树形（父进程缩进），`Space` 折叠/展开，`s` 轮换排序键
- 端口页与进程页：`Enter` 详情（端口给持有进程 / PID / 用户 / 对端，
  内核报告队列长度时一并给出；进程给命令行与资源占用），`k` 杀掉
  （先出计划确认），`p` 直接看选中进程持有的端口
- Disks：挂载表 + 启动目录的用量树，后台扫描、`Enter` 折叠
- `f` 过滤当前页；`r` 手动刷新；`q` 退出

键位行为由终端无关的单元测试钉住（App 状态机 + ratatui TestBackend
渲染断言），另有 Windows 控制台实机验证（含退出码）。

### 通用能力

- 输出格式：表格（默认）、`--json`、`--plain`（制表符、无表头，供脚本）
- 颜色：TTY 自动着色，`--color` / `--no-color` 可强制
- 稳定退出码：0 成功（`port check`：端口被占用）、1 一般错误
  （`port check`：端口空闲）、3 未找到、4 权限不足、5 参数错误、
  130 用户拒绝确认
- 破坏性操作（kill、服务操作）一律先展示计划并确认
- 提权建议按真实主机措辞：权限错误除退出码外还打印 `hint:` 一行，
  Windows 给控制台提权路径（Win+X → Terminal (Admin)、
  `net localgroup Administrators`），Linux 给 sudoers 补救
  （`usermod -aG sudo $USER`），macOS 在 sudo 之外提示隐私与安全
  （完全磁盘访问）；Unix 形状的 Root 要求在 Windows 上不会让用户
  去敲 `sudo`
- `--version-info` 打印适配器与契约版本

## 脚本示例

```sh
# 所有监听 socket，JSON 输出
x port --json

# 制表符输出给 awk / cut
x ps --plain --limit 10

# 端口是否被占用
x port check 8443 || echo "空闲"
```

## 目录结构

| 目录 | 职责 |
| --- | --- |
| `crates/x-core` | 模型、构建器、过滤、kill 计划、稳定错误、测试替身。不含任何平台知识。 |
| `crates/x-platform` | 每个操作系统一个适配器（`macos`、`linux`、`windows`），由 `create_context()` 构建。 |
| `crates/x-cli` | Clap 命令语法、渲染器、确认交互、命令实现。 |
| `crates/x-tui` | Ratatui / crossterm 前端。 |
| `crates/x-app` | `x` 二进制：唯一构建平台上下文的地方。 |

## 开发

```sh
cargo fmt --all
cargo clippy --all-targets --all-features
cargo test --workspace
```

提交前跑本地 CI 检查（Format / Clippy / Test，对应 `ci.yml`）：
`scripts/check.sh`（bash）或 `scripts/check.ps1`（Windows PowerShell）。

交叉编译检查：

```sh
cargo check --target x86_64-unknown-linux-gnu -p x-platform
cargo check --target x86_64-pc-windows-msvc -p x-platform
```

GitHub Actions CI 在 Ubuntu / macOS / Windows 三平台上跑格式、clippy
（零警告）、全量测试，并交叉检查 `x-platform` 的另外两个目标。

## 路线图

**产品原则：不强行统一不存在的能力。** 各平台取公共集合作为基础词汇（Service、Disk、Process…），平台特有能力保留 `x <capability> native …` 逃生舱。CI 已覆盖三平台编译与测试，本地原生验证依赖各平台主机。

### P0 核心价值（数据补全）

基础能力（system / process / port / network / disk / service）已实现，剩余补全项：

- [x] 系统信息：上次重启时间、时区、locale、用户名 / shell / 终端
- [x] CPU：Load Average、当前/最大频率、governor / 电源模式（Linux）、P/E 核心（支持的平台）、温度（能获取时）
- [x] 内存：Swap / Pagefile、内存压力（memory pressure）
- [x] 进程：CWD、线程数、打开的文件、进程级网络连接、环境变量、进程状态；`ps tree` 支持展开/折叠
- [x] 端口：远程地址展示；按进程反查端口（`x port find <name>`）；`x port watch` 持续监视、只在变化时输出
- [x] 网络：网卡链路速度、网关；`x net connections`（统一 netstat / ss / lsof -i，支持按进程/端口过滤）；DNS 解析 / 反查 / 刷缓存；ping / trace / resolve
- [x] 磁盘：物理盘 / 分区 / UUID / 标签 / 只读标志；目录占用（`x disk usage <path> [--depth N]`，du 风格，TUI disk 页签树形展开/折叠）；网卡收发流量统计
- [x] `x net addresses` 的 DHCP 标记（Windows/macOS 已接入；Linux 内核不记录地址来源，诚实留空）
- [x] 服务：`x service logs <name> [--lines N]`（journalctl / macOS 统一日志 / Windows 事件日志）；平台原生操作逃生舱 `x service native <args>…`（systemctl / launchctl / sc 直通）
- [x] 能力探测（`x capability`）：按平台报告各能力 支持/降级/不支持（实测环回 + 只读探测，破坏性项不执行）
- [x] 审计：破坏性操作（进程 kill、端口回收、服务动作、原生逃生舱）逐条写入本地 JSON-lines 日志；`X_AUDIT_PATH` 改路径、`X_AUDIT=off` 关闭

### P1 开发者日常

- [x] 文件：`x file info / type / permissions / owner / size`；`open / reveal / trash / copy / move / rename`（`x file reveal` 优先）
- [x] 环境与 PATH：`x env list/get/set`、`x path list/find/add/remove`、`x which <cmd>`（统一 where / which / command -v）
- [x] Shell：`x shell info / list / default`
- [x] 用户与组：`x user current/list/info`（UID/GID/home/groups/会话）、`x group list/info/members`
- [x] 剪贴板：`x clipboard get/set/clear`
- [x] SSH：`x ssh hosts/connect/test/ping`，读取 `~/.ssh/config`、known_hosts、keys
- [x] Git（可先做插件）：`x git status/branches/changed/conflicts/root`
- [x] 开发环境检测（`x dev` / `x dev node`）：Node / npm / pnpm / yarn / Bun / Deno / Rust / Python / Go / Java / Docker / Git / SSH
- [x] 命令体检（`x doctor <cmd>`）：依赖命令是否存在、版本、路径
- [x] 端口 → 进程 → 项目推断：`x port <n>` 额外给出 CWD、Git 仓库、项目类型与启动命令
- [x] 项目检测（`x project` / `x project info`）：自动识别 Git / Node / Rust / Python / Go / Java 项目、包管理器、分支
- [x] Docker / 容器（插件形态）：`x docker ps/images/ports/logs/port <n>`；未来 Podman / containerd 统一到 `x container …`
- [x] 系统体检（`x doctor`）：一次性检查网络 / DNS / 代理 / Git / Node / Rust / Docker / SSH 等，输出 ✓/⚠

### P2 系统管理

- [x] 启动项（`x startup list/enable/disable`）：Windows Startup / 注册表 Run / 计划任务；Linux systemd / .desktop；macOS LaunchAgents / LaunchDaemons
- [x] 计划任务（`x schedule list/add/remove`）：Windows Task Scheduler / Linux cron、systemd timer / macOS launchd
- [x] 挂载（`x mount list/info/mount/unmount`）：统一 mount / umount / diskutil / net use
- [x] 权限（`x permission <path>` / `check`）：Windows ACL / Unix 权限 / POSIX ACL
- [x] 防火墙（`x firewall status/list/allow/deny`）：Windows Firewall / nftables、iptables、ufw / macOS pf（权限与破坏性操作需谨慎处理）
- [x] 代理（`x proxy get/set/clear`）：HTTP_PROXY / HTTPS_PROXY / NO_PROXY 与系统级代理
- [x] 主机名（`x hosts list/get/add/remove`）：统一 /etc/hosts 与 Windows hosts
- [x] 电源与时间：`x power battery/sleep/shutdown/reboot`、`x time / timezone / sync`
- [x] 日志（`x logs [service|process]`）：journalctl / Event Viewer / log stream（三套体系差异大，需谨慎设计）
- [x] 证书与网络诊断：`x cert check <host>`（Issuer/Subject/有效期/SAN/TLS）、`x http / tls / headers`；综合诊断 `x net check <host>`（DNS → TCP → TLS → 证书 → HTTP；`x doctor` 已被开发者体检占用，链式诊断改挂 net 域）
- [x] 权限提升细化：统一 PermissionRequired / PermissionDenied / ElevationFailed，按平台给出更具体的提权/排障建议

### P3 高级系统能力

- [x] 设备（`x device list/usb/audio/display/…`）：USB、蓝牙、音频、显示、摄像头、键鼠（HID）、网卡；三平台原文透传 + 保守归类
- [x] 蓝牙（`x bluetooth devices/scan/connect/disconnect`）：读三平台原文透传，动词 Linux 全量、Windows/macOS 如实不支持；确认 + 审计
- [x] 显示（`x display list/info`）：分辨率、刷新率、缩放、主显示器、位置；HDR 未提供（Windows 需再过 QueryDisplayConfig，macOS/Linux 口径不一，留待后续如实读取）
- [x] 窗口（`x window list/active/focus/minimize/maximize`）：Windows user32 原生全量；Linux 借 wmctrl/xprop；macOS 免权限列窗口 + System Events 动词（屏幕录制 / 辅助功能按需索取，拒绝原样透出）；动词确认 + 审计
- [x] 事件（`x events`）：进程启停、网络连接、USB 插拔、磁盘挂载、服务状态变化；快照对拍而非内核订阅（x-core 零平台 cfg），家族采样失败如实报告并重置基线，USB 因 Windows 采样约 1.6s 缺省按需开启
- [x] TUI 增强：Dashboard 首页（CPU / 内存 / 磁盘 / 端口概览 + 侧边导航）
- [x] TUI 增强：命令面板（Ctrl+P，可搜索功能而非仅输入命令）
- [x] TUI 增强：全局搜索（`/` 覆盖 process / port / service / network / file）
- [x] TUI 增强：进程管理器（Enter 详情、k 杀掉、t 树形、/ 搜索、s 排序、r 刷新）
- [x] TUI 增强：端口管理器（Enter 展示 进程/PID/CWD/启动命令 + [Kill]）
- [x] TUI 增强：网络页（网卡状态/IP/MAC/速度 + 实时连接表）
- [x] TUI 增强：树形展开/折叠、Disks 与 Services 页签、跨页签联动（选中进程直接看它持有的端口）
- [x] 更多内核级数据：`x port stats` 按状态 / 协议统计连接；socket 队列长度（Linux 读写两侧、macOS 仅 TCP 发送侧、Windows 如实无此数据，None 不入统计）

### P4 平台化

- [ ] 远程模式：`x remote server` / `x remote connect <host>`（远端装有 x 时经 SSH 直连），TUI 可在 Local / server-a / server-b 间切换
- [ ] 插件系统：`x plugins list/install`，可扩展命令、TUI 页签、系统提供器、输出格式化器
- [ ] MCP Server：暴露 find_port / find_process / list_services 等工具给 AI Agent，危险操作保留明确确认
- [ ] Agent 模式：`x explain <命令>` 解释输出；自然语言输入放最后，核心不依赖 AI
- [ ] 配置（`~/.config/x/config.toml`）：主题、刷新率、默认输出格式、默认排序
- [ ] 主题：TUI 内置 Default / Dark / Light / Monochrome + 自定义
- [ ] 输出：`--jsonl`、`--csv`（human / json 已有）
- [ ] shell 补全：`x completion bash|zsh|fish|powershell|elvish`
- [ ] man 页与帮助生成
- [ ] Dry run：`--dry-run` 先说明将执行的操作（与确认机制互补）
- [ ] 打包分发：install 脚本 / Homebrew tap / Windows 安装包
- [ ] 各平台原生运行时的持续回归（CI 已覆盖，需结合真实故障复现）
