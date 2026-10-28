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

## 安装

### 从 GitHub Release 安装（推荐）

一条命令下载最新版并装进 PATH（无需 Rust 工具链）：

macOS / Linux：

```sh
curl -fsSL https://raw.githubusercontent.com/wangmingfa/x/main/scripts/install-release.sh | sh
```

Windows (PowerShell)：

```powershell
powershell -c "irm https://raw.githubusercontent.com/wangmingfa/x/main/scripts/install-release.ps1|iex"
```

- 脚本按平台自动选产物：macOS 用 universal 包（Apple Silicon 与 Intel 合在一个
  二进制里，无需判架构）；Linux 按 `uname -m` 取 `Linux-x86_64` / `Linux-aarch64`；
  Windows 下载 Inno 安装包静默安装。装到 `PREFIX/bin`（默认 `/usr/local/bin`；
  Windows 由安装包落位到 `%LOCALAPPDATA%\x`），`-AddToPath` 把该目录写进用户 PATH。
- 装指定版本：`X_VERSION=v0.1.2`（bash）或 `-Version v0.1.2`（PowerShell）。
- 注：把安装目录写进用户 PATH 后，**需重开终端（或新开一个窗口）**，`x` 命令才会
  生效——已打开的终端不会自动读取新的 PATH。Windows 安装脚本结束时也会打印该提示。

### 升级

`x upgrade` 从 GitHub Release 拉取最新版本并替换自身（无需重新跑安装脚本）：

```sh
x upgrade
```

- 自动按平台选产物（macOS universal / Linux 按架构 / Windows 安装包），与
  安装脚本同一套逻辑；最新 release 是预发布版时同样可取。
- 已是最新版则提示 "already up to date"；`--force` 强制重装同版本。
- 替换自身属破坏性动作，先确认（`--yes` 跳过，拒绝退出码 130）。
- Windows 上通过重命名技巧替换运行中的 `x.exe`（旧文件改名 `x.exe.old`）。

### 从源码构建安装

需要 Rust 工具链：

macOS / Linux：

```sh
curl -fsSL https://raw.githubusercontent.com/wangmingfa/x/main/scripts/install.sh | sh
```

Windows (PowerShell)：

```powershell
powershell -c "irm https://raw.githubusercontent.com/wangmingfa/x/main/scripts/install.ps1|iex"
```

脚本会克隆仓库、`cargo build --release -p x-app` 并把二进制拷到 `PREFIX/bin`
（默认 `/usr/local/bin`；Windows 默认 `%LOCALAPPDATA%\x`），并提示把该目录加入 PATH。

- **Homebrew**：见 [`Packaging/Homebrew/README.md`](Packaging/Homebrew/README.md)，
  `brew tap xsys/tap && brew install --head x`。
- **直接下载**：GitHub Release 上每个 tag 带五个文件——`x-<tag>-Linux-x86_64.tar.gz`、
  `x-<tag>-Linux-aarch64.tar.gz`、`x-<tag>-macOS-universal.tar.gz`（Apple Silicon 与
  Intel 合在一个二进制里）、`x-<tag>-windows-x86_64.tar.gz`（裸二进制，供 `x upgrade`
  自我升级）、`x-<tag>-windows-x86_64-setup.exe`。
- **Windows 安装包**：见 [`Packaging/Windows/x.iss`](Packaging/Windows/x.iss)
  （Inno Setup 脚本，CI 在打 tag 时自动编译成 `x-*-windows-x86_64-setup.exe`）。
- 打 tag 触发 `.github/workflows/release.yml` 构建上面四个产物并发布 GitHub Release；
  Release 正文由 `scripts/changelog.sh --release` 生成，与 [`CHANGELOG.md`](CHANGELOG.md) 同源。
  构建镜像是写死的版本号（`ubuntu-22.04` / `ubuntu-24.04-arm` / `macos-15` / `windows-2022`），
  不是 `-latest`：产物能跑在哪些机器上，取决于构建机当年的 glibc 和 SDK，`-latest` 会在
  你不改一行代码的时候把这个下限挪走。publish 作业会把正文里点名的文件和实际构建出的文件
  逐一对比，不一致就不发布。

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
| `x ps watch` | 持续轮询，只输出进程增删（`--interval`、`--count`） |

### 系统（`x sys`）

| 命令 | 说明 |
| --- | --- |
| `x sys info` | 操作系统、内核、架构、主机名、CPU 型号、核数（含 P/E 核心划分）、内存、运行时长、上次重启、时区、locale、用户 / shell / 终端 |
| `x sys cpu` | 聚合与每核 CPU 利用率，负载均值、当前/最大频率（平台提供时）、温度（Linux thermal_zone/hwmon、macOS SMC）、governor（Linux） |
| `x sys mem` | 内存利用率 |
| `x sys watch` | 持续采样 CPU / 内存，每采样一行（`--interval`、`--count`） |

### 网络（`x net`）

| 命令 | 说明 |
| --- | --- |
| `x net interfaces` | 网卡：状态、MAC、MTU、链路速度、收发流量、默认网关、默认路由标记、链路类型 |
| `x net addresses` | IP 地址 + 前缀长度（来自 netmask）+ DHCP 标记 |
| `x net routes` | 路由表（IPv4/IPv6） |
| `x net dns` | DNS 解析器配置 |
| `x net flush` | 刷新系统 DNS 缓存 |
| `x net connections` | 全部套接字 + 归属进程，`--process` / `--port` 过滤 |
| `x net top` | 每进程实时网速：RX/TX 速率、连接数与来源标注（`--interval` / `--count` / `--pid` / `--sort` / `--json`）；抓包需 root，无权限时如实降级为连接归属 + 整机速率并给 hint |
| `x net resolve <host>` | 正向解析：主机名 → 地址 |
| `x net reverse <ip>` | 反向解析：地址 → 主机名 |
| `x net ping <host>` | ICMP 连通性（`--count` / `--timeout`），输出 min/avg/max |
| `x net trace <host>` | 逐跳路径（`--max-hops` / `--timeout`），原生 ICMP，不依赖 tracert |
| `x net check <host>` | 链式诊断：DNS → TCP → TLS → 证书 → HTTP，任一环失败如实标注 |
| `x net speed [url]` | 实测下载吞吐（`--count` 多次采样、报告峰值；`--timeout`），与 interfaces 的协商速率分列 |

### 磁盘（`x disk`）

| 命令 | 说明 |
| --- | --- |
| `x disk list` | 全部挂载文件系统：设备、类型、介质、容量、已用、可用、使用率（`--json` 含标签 / 卷 UUID / 分区 UUID / 型号 / 序列号 / 只读） |
| `x disk current` | 当前目录所在的文件系统 |
| `x disk usage <path> [--depth N]` | 目录占用，du 风格：按聚合大小排序，`--depth` 限制展示层级，路径不存在退出码 3 |
| `x disk io` | 块设备 I/O 速率：每整盘的读/写字节与 IOPS，按间隔采样差值（`--interval` / `--count` / `--sort` / `--json`）；累计计数 → 区间速率，与 `x net top` 同构。Windows 未接入 `IOCTL_STORAGE_QUERY_PERF_DATA` 绑定，如实报退出码 7 |

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
| 上述三者 + `--follow` | 持续跟随，只输出新增记录（`--interval`、`--count`）；按整条记录内容去重 |
| 上述三者 + `--grep` / `--level` | 过滤：文本命中 message/origin/level（不分大小写）；`--level` 逗号分隔，词汇按平台原文（journald `err`、Windows `Error`、unified log `error`） |
| 上述三者 + `--context N` | 命中行各带 N 行上下文（-B/-A 语义，按窗口截断）；表格多一列 `match` 标命中，`--follow` 下只影响首轮展示、不动增量判定 |

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

### 性能基线（`x bench`）

| 命令 | 说明 |
| --- | --- |
| `x bench` | 逐个实测列表类读操作的耗时：最快值 / 中位数 / 每行微秒数，对照预算给出 ok / slow / failed |
| `x bench --samples <n>` | 每个目标计时的轮数（默认 3）；另加一轮**不计时**的预热，避免把冷缓存读成适配器慢 |
| `x bench --domain <process\|port\|net\|disk\|service>` | 只测一个域，与 `x capability --domain` 同一套域名 |
| `x bench --budget-ms <ms>` | 用同一个预算覆盖各目标自带的预算 |
| `x bench --check` | 有目标超预算时退出 1，供 CI 当回归闸门 |

预算是**宽松的天花板**，不是性能目标：它要抓的是「某个适配器悄悄多做了一次
系统调用、或者分组从线性变成了平方」这种量级变化，而不是给机器打分。
每行微秒数按适配器**实际返回的行数**归一，所以 300 进程的笔记本和 3000
进程的服务器仍可比。本机行数少于基线规模（路线图说的 1000 进程 /
10k socket）时，输出会逐目标说明这一点的行数和基线规模，并建议与本机
历史而不是别的机器比较——不假装绝对毫秒数是可比的。适配器报错的目标
记为 `failed` 并保留它自己的错误消息，不写成 0 行 0 毫秒的"很快"。

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

读回来：`x audit list [--action A] [--user U] [--failures] [--since T]
[--grep S] [--limit N] [--path P]`，最新在前。`--action` 支持整域通配
（`process.*`）；日志按行去重无关，读侧只解析自家写的 JSON lines，所以
三平台形状一致。半行截断（写入途中被杀）与坏行如实计数并在输出末尾说明，
不因为一条坏记录让整份日志不可读；日志不存在即「没有记录」而非报错。

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
  （`port check`：端口空闲、`x bench --check`：有目标超预算）、3 未找到、
  4 权限不足、5 参数错误、130 用户拒绝确认
- 破坏性操作的确认契约（所有命令一致，契约测试钉死）：
  1. `--dry-run` 恒最先——打印计划、退出 0、绝不提问；JSON 调用方得到
     单个 JSON 文档（`{"dry_run": true, "would": …}`），不会有散文混进流
  2. JSON 不豁免确认——输出格式与是否突变正交；JSON 模式下被拒的
     操作返回单个 `{"aborted": true, …}` 文档
  3. 拒绝确认一律退出 130——拒绝是脚本流程控制，不是输入错误
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

# 列表类适配器有没有变慢（CI 闸门）
x bench --check --samples 3 || echo "有读操作超预算"
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

从仓库直接跑，不用先安装（`x-app` 的产物名是 `x`）：

```sh
cargo run -p x-app -- port                # 等价于 target/debug/x port
cargo run -p x-app -- ps --json
cargo run -p x-app -- ui                  # 交互式界面，需要真实 TTY
cargo run --release -p x-app -- sys info  # 看 release 下的真实耗时
```

`--` 之后才是 `x` 自己的参数。想看某个子命令的全部选项，用
`cargo run -p x-app -- port --help`。界面类命令（`ui` / `tui`）要求终端是 TTY，
在 CI 或重定向到管道时会直接报错退出，这是刻意的——一次性输出请用对应子命令。

检查：

```sh
cargo fmt --all
cargo clippy --all-targets --all-features
cargo test --workspace
```

提交前跑本地 CI 检查（Format / Clippy / Test，对应 `ci.yml`）：
`scripts/check.sh`（bash）或 `scripts/check.ps1`（Windows PowerShell）。
`check.ps1` 带 shebang，Git Bash 里 `./scripts/check.ps1` 会交给
`powershell.exe -File` 执行，回传的退出码就是检查自己产生的那个。

交叉编译检查：

```sh
cargo check --target x86_64-unknown-linux-gnu -p x-platform
cargo check --target x86_64-pc-windows-msvc -p x-platform
```

### 发布

发布入口只有一个：`scripts/release-tag.sh`。CHANGELOG 与 GitHub Release 正文由
`scripts/changelog.sh` 从提交历史生成，两者同源，所以仓库里的 CHANGELOG 与
发布页面上的说明不会各自漂移：

```sh
scripts/changelog.sh                       # 整份历史，markdown 打到 stdout
scripts/changelog.sh -o CHANGELOG.md       # 写文件
scripts/changelog.sh --heading v0.1.2      # 把待发布一节命名为该版本号
scripts/changelog.sh --release v0.1.2      # 只出这个 tag 区间的 Release 正文
```

`release-tag.sh` 在打 tag 之前先用 `--heading "$tag"` 刷新 `CHANGELOG.md`；文件
因此变化就停下来，把该提交的命令原样打给你，等你提交后再跑一遍。脚本不替你提交，
tag 因此只会指向你看过并同意的那棵树。而生成器不会把 `docs: regenerate CHANGELOG …`
这条自己的产物列进条目——否则「提交 changelog → 多出一条 changelog → 又要提交」
永远收不住。

`.github/workflows/release.yml` 的 publish 作业跑
`scripts/changelog.sh --release <tag> -o release-notes.md` 并把结果作为 Release
正文；它的 checkout 带 `fetch-depth: 0` 与 `fetch-tags: true`，否则读不到 tag 区间。
publish 还有一道自检：把正文里点名的产物与实际构建出的文件逐一对比，不一致就退出——
说明里 promise 一个下载页上没有的文件，比构建失败更糟。
Windows 安装包的 `ReleaseTag` 也由工作流传给 Inno，`x-v0.1.2-windows-x86_64-setup.exe`
在文件系统和在说明里是同一个名字。

构建矩阵的镜像是固定版本号而不是 `-latest`：`ubuntu-22.04`（x86_64）、
`ubuntu-24.04-arm`（aarch64，GitHub 只给到这一档 ARM Linux runner）、`macos-15`
（GitHub 已没有 Intel macOS runner，x86_64 那一半在同一台 arm64 机上交叉编译，再用
`lipo` 合成 universal）、`windows-2022`。固定的是**构建机**，不是「产物要求的系统」——
纯 Rust 二进制引用的 glibc 符号常常比构建机自带的那版更老，所以真正的下限别看镜像名，
看构建日志：Linux 那一步会把产物里引用到的最高几个 `GLIBC_*` 打出来，macOS 那一步打
`lipo -archs` 和产物自己声明的 `minos`。

GitHub Actions CI 在 Ubuntu / macOS / Windows 三平台上跑格式、clippy
（零警告）、全量测试，并交叉检查 `x-platform` 的另外两个目标。

## 路线图

**产品原则：不强行统一不存在的能力。** 各平台取公共集合作为基础词汇（Service、Disk、Process…），平台特有能力保留 `x <capability> native …` 逃生舱。

完整的路线图（P0 核心价值 → P5 候选池，含各项的落地取舍）见
[`docs/roadmap.md`](docs/roadmap.md)。
