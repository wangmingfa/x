# x 路线图

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
- [x] `x net addresses` 的 DHCP 标记（Windows/macOS 已接入；Linux 内核不记录地址来源，改取用户态的权威来源——`ip -4 addr show` 的 `dynamic` 与 `nmcli` 的 `auto` 取并集，两者都没提到的接口留空而不是当成静态）
- [x] 服务：`x service logs <name> [--lines N]`（journalctl / macOS 统一日志 / Windows 事件日志）；平台原生操作逃生舱 `x service native <args>…`（systemctl / launchctl / sc 直通）
- [x] 能力探测（`x capability`）：按平台报告各能力 支持/降级/不支持（实测环回 + 只读探测，破坏性项不执行）
- [x] 审计：破坏性操作（进程 kill、端口回收、服务动作、原生逃生舱）逐条写入本地 JSON-lines 日志；`X_AUDIT_PATH` 改路径、`X_AUDIT=off` 关闭

### P1 开发者日常

- [x] 文件：`x file info / type / permissions / owner / size / hash`；`open / reveal / trash / copy / move / rename`（`x file reveal` 优先）；`wait`（`--for appear|change|stable`，超时退出 3）
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
- [x] 显示（`x display list/info`）：分辨率、刷新率、缩放、主显示器、位置；HDR 按三态如实读取（Windows 再过一次 DisplayConfig 取 `GET_ADVANCED_COLOR_INFO`，macOS / Linux 取决于 EDID 与合成器、x 无从得知，留「没说」而不是「没开」）
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

- [x] 远程模式：`x remote server` / `x remote connect <host>`（远端装有 x 时经 SSH 直连），TUI 可在 Local / server-a / server-b 间切换
- [x] 插件系统：`x plugins list/install`，可扩展命令、TUI 页签、系统提供器、输出格式化器
- [x] MCP Server：暴露 find_port / find_process / list_services 等工具给 AI Agent，危险操作保留明确确认
- [x] Agent 模式：`x explain <命令>` 解释输出；自然语言输入放最后，核心不依赖 AI
- [x] 配置（`~/.config/x/config.toml`）：主题、刷新率、默认输出格式、默认排序、TUI 键位
- [x] 主题：TUI 内置 Default / Dark / Light / Monochrome + 自定义
- [x] 输出：`--jsonl`、`--csv`（human / json 已有）
- [x] shell 补全：`x completion bash|zsh|fish|powershell|elvish`
- [x] man 页与帮助生成
- [x] Dry run：`--dry-run` 先说明将执行的操作（与确认机制互补）
- [x] 打包分发：install 脚本 / Homebrew tap / Windows 安装包
- [x] 各平台原生运行时的持续回归（CI 已覆盖，新增 `regression` 测试组 + 真实原生冒烟：覆盖权限拒绝、目标消失、服务管理器不可达、磁盘/网络读失败等真实故障，确认优雅降级而非崩溃）

### P5 候选（待启动，按性价比排序）

下面是从当前代码面（已实现的命令面、x-core 模型层、平台门控现状）梳理出的
候选，不预先承诺顺序；每项开工前仍按「不强行统一不存在的能力」原则复核
平台可得性。

**A 组 · 高性价比，缺口明确**

- [x] 实时监视：`x ps watch` / `x sys watch`（对齐已有的 `x port watch` 与
  `x events` 轮询模型：定时重采样、Ctrl+C 干净退出，`--interval` / `--count`）。
  `ps watch` 只输出进程增删（新增 `x_core::diff_processes`，按 pid 比对、两侧
  排序稳定）；`sys watch` 每采样输出一行——利用率仪表是时间序列，做 diff 会把
  最该看的尖峰抹掉。采样失败只报错并保留上次基线：瞬时权限错误不该被读成
  「所有进程都退出了」。
- [x] 日志跟随：`x logs system|service|process --follow`，加 `--grep` / `--level`
  过滤（`--interval`、`--count`）。**`--since` 刻意不做**：三套日志的时间戳文本
  互不可比，把它们解析成一个过滤器正是本项目拒绝的有损假统一。轮询去重按整条
  记录内容而非时间戳——同一时刻的多条记录很常见，没有稳定记录 id 时抑制逐字
  重复就是诚实结果。首轮严格只出 `--limit` 行，加宽窗口仅用于探测新增。
- [x] 文件哈希：`x file hash <path> [--algo sha256|sha512|crc32]`（纯读取，三平台
  同构，std 无依赖手写实现，不新增任何依赖）。流式读取，大文件不落内存；输出
  用 `sha256sum` 的 `<hex>  <path>` 形状便于与平台工具对拍。算法钉在公开测试向量
  上而非自洽——这一点立刻抓到过 SHA-512 常量表里几个丢失的低位 nibble。目录
  如实拒绝：不存在能与 `sha256sum` 对账的「目录摘要」。
- [x] 审计查询：`x audit list [--action A] [--user U] [--failures] [--since T]
  [--grep S] [--limit N] [--path P]`（读自己写的 JSON-lines 日志，天然三平台
  同构；写侧已有、读侧空白，是审计闭环的最后一块）。`--action` 支持 `process.*`
  整域通配。半行截断（写入途中被杀）与坏行如实计数并在输出末尾说明，不因一条坏
  记录让整份日志不可读；日志不存在即「没有记录」而非报错；`--path` 读法同样
  走全部过滤（那是读别处拷来的日志的路径，丢过滤会让 flag 看起来坏了）。
- [x] `x net speed`：链路协商速率与实际吞吐分列（现有 interfaces 只报协商
  速率，README 已注明「不是带宽测量」）。走平台自带 curl 抓一次传输并计时，
  得到的是这条连接真正搬了多少；两个读数刻意分列字段——快链路上的慢采样、
  慢链路上的快采样，正是用户要找的差。`--count n` 逐次报而不是求平均（冷启动
  与热缓存回答的是不同问题），末尾附峰值。curl 把耗时报成 `0.000000` 时如实
  拒绝，不做除法（否则会打印无限速率）。

**B 组 · 补齐已知留空项**

- [x] macOS CPU 温度：`x sys cpu` 的温度此前仅 Linux 有（thermal_zone/hwmon），
  现经 IOKit 连接 `AppleSMC` 用户客户端读取 CPU 传感器。**枚举键名而不是硬编码**：
  Intel 是 `TC0*`/`Tm0*`；本机（Mac16,9）在 2109 个键的实测键表里是四个 die 簇
  `Tp0*`(45)/`Tp1*`(14)/`Tp2*`(31)/`Tpx*`(12) 加 `Ts0*`(30) 热管簇，共 132 个键。任何
  固定清单都只在部分机型上成立，而只认 `Tp0*` 会少报：同一时刻 `Tp1o` 比最高的 `Tp0*`
  高 3.84 ℃（82.109375 对 78.265625）。GPU（`TG0*`）、环境（`TA0*`）、电池（`TB0*`）、
  内存（大写 `TM0P`）都排除——那是别的传感器；形近的 `TPD0`/`TPMp`/`TPSD`/`TS0p`/`TSV*`/
  `TMVR`/`TH0x`/`TCDX`/`TCMb`/`TCMz` 同样排除，大小写在这里有意义（小写 `Tm0` 是核心，
  大写 `TM0P` 是内存）。`Tsx0`/`Tsx1` 读数与 `Ts0*` 同段但不在这次的口径决定里，不读。
  **按 `keyInfo` 声明的类型分流解码**：`sp78` 是
  **大端有符号** 8.8 定点，`flt ` 是**小端 IEEE-754 单精度**——本机实测 132 个 CPU 键
  清一色 `flt `、`sp78` 键一个都没有，只认一种类型就等于在 Apple silicon 上永远读不到。
  类型不是装饰：同一串 `00 20 83 42` 按 `flt ` 是 65.5625 ℃、按 `sp78` 是 0.125 ℃，两个
  都落在 −40…150 ℃ 的窗口里，选错解码器拿到的是一个看不出破绽的错数。认不出的类型
  （`ioft`、`ui8 `…）不解码，宁缺不猜；未接传感器的典型回答（`sp78` 下限 `0x8000`，即
  −128.0 ℃）按窗口丢弃。多数机器上无特权会被拒绝、虚拟机可能没有 CPU 传感器键，都如实
  返回 `None`，`x capability` 早已把 `None` 渲染成「本平台未暴露」——不猜值、不估算。
  两笔 SIGBUS 的教训值得留档：`IOConnectCallStructMethod` 的大小参数是 `size_t` 不是
  `u32`，传窄了高半截寄存器是垃圾值，内核按错的尺寸往 56 字节的结构体里拷 80 字节；
  `mach_task_self_` 是 libsystem_kernel 的**数据符号**（C 里 `mach_task_self()` 只是读它
  的宏），声明成函数照样链接通过，运行时跳进 `__DATA` 把 task port 的数值当指令执行。
  两处共同点：都不返回错误码，直接把内存或控制流改掉，所以「调用失败」这种期待本身
  不成立；协议结构按权威定义写成 80 字节，布局用编译期断言钉死。**IOKit 那一层在 CI 上
  仍是空跑的**：GitHub 的 macOS runner 是虚拟机，没有 `AppleSMC` 服务，`Smc::open()` 在
  匹配服务那步就返回 `None`。所以缝抽在**一次 80 字节交换**上（`RoundTrip`）：计数、枚举、
  过滤、按类型分流、按簇聚合判断整条循环都搬进平台无关模块，用这台机器录下的真实 transcript
  回放（`crates/x-platform/tests/fixtures/smc/`：`#KEY`=2109 与全部 2109 个键名，加 132 个
  CPU 键的 keyinfo 与原始载荷；三份捕获分别是一次全簇分化、一次四个 die 簇全占位而热管簇照旧
  报温度、一次 45 键整组同值 40.0 的塌缩窗口）——CI 三平台跑的就是
  产品代码里的那条循环，`macos/smc.rs` 只剩 FFI。真机取证过程见
  `docs/devlog/2026-10-09-macos-smc-mach-task-self.md`。**按簇判分化，没分化的那一簇一个数都
  不留**：占位是**一簇一簇**发生的——实测同一次读取里 `Tp0*` 45 键只有 5 个不同值、`Tp1*` 4/14、
  `Tp2*` 5/31、`Tpx*` 1/12，而 `Ts0*` 同期报 20 个不同温度；这些占位值全在合理窗口内（本机
  见过的形状是 ≤5.2 ℃ 和整组 40.0），单独看都像真读数。随机交替实验（147 个样本）排除了
  「新开 client 的前几轮」和「空闲后缓存过期」两种解释（同一 client 内再读一遍、复用长命 client
  都不能避免），所以防护按**回答的内容**判——某簇的不同值不足该簇键数一半时整簇丢弃（单键机器
  不受影响）。口径放宽后不能再「全机一把判」：132 键合并阈值是 66 个不同值，而 50 次真实捕获里
  最省的一次只数到 69——3 个值的余量；分簇判之后每个簇对自己的一半都有至少 4 的余量（最紧的是
  `Ts0*`：需要 15，实测 19–20）。可见后果：die 簇全占位时 x 不再沉默，而是报热管簇那 46–54 ℃，
  且本机 50 次捕获里 `Ts0*` 每次都分化，所以「本平台未暴露」在这台机器上此后基本只在无特权或
  虚拟机上出现。判断与它喂进去的真实载荷都在平台无关模块里，CI 三平台都真跑；机制本身仍未定论，见同一篇 devlog。
- [x] 容器统一到 `x container`：新增 `x container ps/images/ports/port/logs`，
  自动发现本机引擎（docker → podman → nerdctl），`x container engines` 列出可用
  与当前引擎，`X_CONTAINER_ENGINE` 可钉住。三套 CLI 各一个适配，**不强行合并成假
  统一**：每一行都带 `engine` 字段（同机装了 docker 和 podman 时同一个 id 可能属
  于不同容器），状态/端口一律保留引擎原文，不改写。`x docker` 保留为别名但**钉死**
  docker，绝不会因为装了 podman 就改从 podman 回答。原有的 `dockerinfo` 模块随之
  删除——留下两套并行容器层与「统一」目标自相矛盾。探测用 `info`：二进制缺失与
  后端未启动都能一次分清；拼错的 `X_CONTAINER_ENGINE` 报参数错误（退出码 5）而不是
  谎称「本机没有引擎」。
- [x] 显示 HDR：`DisplayInfo` 新增 `hdr_enabled` 三态（`Some(true)` 现在开着、
  `Some(false)` 支持但没开、`None` 平台/本机没说——**「没说」不等于「没开」**，把无
  HDR 概念的机器显示成 `no` 是没人能核对的谎）。Windows 走 DisplayConfig 的
  `DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO`（GDI 的 `DEVMODE` 根本没有这个字段），且
  仅在 `colorEncoding` 为 RGB 时才读那个 bit。**关联键选 `viewGdiDeviceName`**：
  DisplayConfig 的 LUID 键在 GDI 遍历那侧根本拿不到，用了会静默匹配不到任何东西——
  而「一直匹配不到」和「一堆 SDR 屏」在输出上完全无法分辨。macOS / Linux 的 HDR 状态
  取决于 EDID 与合成器，x 无从得知，故留 `None`（`x display list` 的 `hdr` 列显示
  `-`）。`x capability` 新增 display 域：按每块屏统计如实报 supported / degraded /
  unsupported，而不是整机一个 yes/no。
- [x] DHCP 标记补齐：Linux 内核不记录地址来源，但**用户态有权威来源**，故已接入。
  两个来源取并集：`ip -4 addr show` 给 DHCP 分配的地址打 `dynamic` 标记（内核视角，
  不依赖任何守护进程），`nmcli -g IP4.METHOD,DEVICE con show --active` 报 `auto`
  （NetworkManager 视角，覆盖装好后地址看起来已是静态的配置）。只标记
  `scope global`——link-local 的 `dynamic` 与可路由地址的配置方式无关；IPv6 一律
  不标（SLAAC / DHCPv6 与 DHCPv4 租约无关）。**只记录肯定答案**：两个工具都没提到的
  接口留 `None`（`x net addresses` 该列为空）而不是 `false`——「没工具说过」不等于
  「静态配置」，否则既没装这两个工具、或用第三方 DHCP 客户端的机器都会被误标。

**C 组 · 产品化收尾**

- [x] 破坏性操作的 `--yes` 语义统一化：12 个命令统一到同一契约
  （dry-run 恒最先且绝不提问、JSON 不豁免确认、拒绝一律退出 130），
  契约矩阵测试（`tests/contract.rs`）按命令 × {dry-run, JSON dry-run,
  拒绝, JSON 拒绝} 钉死。
- [x] TUI 键位可配置：`config.toml` 里 `key_<动作> = "字符"` 覆盖默认键位
  （可配置动作：quit、kill、search、filter、refresh、tree、sort、ports、
  top、bottom，如 `key_kill = "x"`）；方向键、Tab、Home/End、翻页、
  Enter、Esc、Ctrl+C、Ctrl+P 与数字跳页保持固定。未知动作名或非单字符
  值在状态行报 warning 并保留默认键位。
- [x] 退出码契约版本化：`docs/contract.md` 显式钉死退出码全表、破坏性
  操作确认契约与 JSON 输出约定，含稳定性承诺；`--version-info` 的
  `contract` 字段与文档版本一致，并新增 `contract_doc` 字段指向文档。
- [x] 性能基线：新增 `x bench`，对 11 个列表类读操作逐目标计时（一轮**不计时**
  的预热 + `--samples` 轮），报最快值 / 中位数 / **每行微秒数**，对照各目标自带
  的预算判 ok / slow / failed；`--check` 把「超预算」变成退出 1 供 CI 当闸门
  （`docs/contract.md` 记了这条特例）。预算是天花板不是性能目标：要抓的是「多了
  一次系统调用、分组悄悄变成平方」这种量级变化，不给机器打分。路线图说的
  「1000 进程 / 10k 端口」规模**不假装能凭空造出来**：本机行数低于该规模时逐目标
  说明实际行数与基线规模，并建议与本机历史比较而不是与别的机器比较；每行微秒数按
  适配器真实返回的行数归一，小机器与大机器因此仍可比。以固定开销为主的读
  （interfaces / addresses / routes / disks / service）没有行规模可言，故不配基线；
  过滤型读（listening）行数少但计时的是整张表，它的行数也不是规模证据。适配器
  报错记 `failed` 并保留它自己的消息，不写成「0 行、很快」；`x ps` 的 usage 采样
  要等 CPU 采样窗口（≥120 ms），这件事写在 note 列里，而不是被读成适配器变慢。
- [x] 发布流程与 CHANGELOG：新增 `scripts/changelog.sh`，从提交历史生成
  `CHANGELOG.md`，`--release <tag>` 生成同一个 tag 区间的 GitHub Release 正文——
  两处同源，措辞与列出的提交不会各自漂移。`release-tag.sh` 打 tag 前先刷新
  `CHANGELOG.md`，文件变了就停下并把要提交的命令打给你：脚本不替你提交，tag 因此
  只会指向确认过的树；生成器把 `docs: regenerate CHANGELOG …` 这条自己的产物排除
  在外，否则「提交 changelog 又生出一条 changelog」永远收不住。分组只认得
  `type:` 前缀，认不出的（`C4 …`、`P4 打包分发：…`）**原样**进「Other changes」，
  不重写也不丢弃——一个看不出漏了什么的 changelog，比一个末尾不整洁的更糟。merge
  提交跳过，`!` 保留 breaking 标记，scope 渲染成 `**port:**`。release.yml 的
  publish 作业改为带 `fetch-depth: 0` + `fetch-tags: true` 检出，notes 才有区间
  可读；Inno 的 `ReleaseTag` 由工作流从 tag 传入（本地编译用文件里的兜底值），
  安装包文件名与 notes 里承诺的资产名因此是同一个。

**D 组 · 候选（2026-10 新增；前两项已实现，余项未开工，按性价比排序）**

- [x] 监视告警：`x port watch` / `x ps watch` 已有轮询骨架但没有动作出口——加
  `--when <条件> --exec <命令>`（如端口被占用/进程消失时触发），或至少 `--quiet-until-change`
  变成可用的高频轮询源。轮询模型、退出码、审计都是现成的，只缺条件判定与触发一跳。
  风险点：触发命令的注入面要按破坏性操作契约过审计，且「触发失败」必须如实报告而不是静默吞掉。
- [x] 句柄/FD 泄漏探测：`x events` 已盯进程/网络/USB/磁盘/服务五类快照对拍，
  补第六类——每进程打开句柄数（macOS `proc_pidinfo`、Linux `/proc/<pid>/fd` 计数、
  Windows `GetProcessHandleCount`），`x ps` 加一列、`x events` 加 drift 事件。
  全是现有采样循环顺手能拿的读数，且是「进程悄悄涨内存/涨 fd」这类问题的唯一定位入口。
- [x] `x disk io`：块设备 I/O 速率（读/写字节与 IOPS，按间隔采样差值）。
  Linux `/proc/diskstats`、macOS `IOService` IOKit 统计、Windows `IO_COUNTERS`——
  三个来源都是纯读数，形状与 `x net top` 的「累计计数 → 区间速率」完全同构，
  x-core 的 diff 模型可直接复用。
  落地时 Windows 侧诚实降级：当前 `windows-sys` 只绑定了 `Threading` 下不相关的
  `IO_COUNTERS`（进程内存结构），`IOCTL_STORAGE_QUERY_PERF_DATA` 的 20 字段布局
  手写错一个字段会静默报错字节数，所以命令在 Windows 报退出码 7 而非造假读数。
  macOS 侧一个坑：空卡槽的 `IOBlockStorageDriver` 子树里没有任何 `IOMedia`，
  于是没有 `BSD Name`；用 registry id 兜底会让名字每轮都变（每次匹配都新建 entry），
  永远匹配不到上一轮、速率恒为 0，所以直接跳过这种条目。
- [x] TUI net top 深化：9 页目前只有表格与 hint，补排序切换（rx/tx/conns）、
  选中进程 Enter 跳转它持有的端口（跨页联动的既有机制）、`--pid` 过滤视图。
  全是 TUI 内部接线，不碰平台层。
  落地形态：`s` 在 net top 页循环 total/rx/tx/conns/name 五种排序（processes 页
  的 `s` 走它自己的循环，互不干扰）；Enter 与 `p` 都跳 ports 页并按选中进程名过滤
  （过滤用进程名而不是 pid——ports 的 search 把 pid 当文本子串，`12` 会连带匹到
  `1234`）；`f` 过滤行，pid 与进程名都认。`(unmapped)` 汇总行没有 pid，Enter 会
  提示而不是跳转，且任何排序列都不会把它顶到真进程上面。
- [x] `x file wait <path>`：阻塞等待文件出现/变化/稳定（`--for appear|change|stable`，
  stable 要求相邻两轮采样的大小与 mtime 一致——「大小两轮不变」），供脚本串联
  （下载完成检测、构建产物等待）。零平台差异，纯轮询语义（`--interval`，缺省 0.5s）。
  超时行为写进退出码而不是都返回 0：条件满足退出 0，`--timeout N` 秒内未满足退出 3
  （与「未找到」共用），非法数值退出 5。`--json` 输出单文档
  （`condition` / `path` / `met` / `waited_ms` / `detail`）。纯读，不落审计。
  change 以首次采样为基线：文件原本不在、之后出现也算一次变化；stable 在文件
  中途消失时重置基线，不会把「回来后的第一轮」误判成稳定。
- [x] 日志 `--context`：`x logs --grep … --context N` 命中后带上下文行（-B/-A 语义，
  两个方向、按取回的窗口截断）。上下文行按窗口原序混排、不重排不改写；表格模式
  加 `match` 列（`*` 命中 / `-` 上下文），JSON 文档的 entries 含上下文行（哪些是
  命中由过滤条件可复算，不另改 LogPage 形状）。轮询去重按「整条记录内容」的现有
  约定不受影响——上下文只影响 `--follow` 首轮窗口的展示，后续轮次照旧只打新增；
  首轮打印行数仍受 `--limit` 约束（上下文行计入该限额）。

