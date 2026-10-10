# `x net top` 设计方案：无外部依赖的每进程网络统计

状态：设计稿（待评审）。对应路线图 P5 候选。

## 1. 问题与结论

需求：像 `nethogs` 一样看到「当前所有进程的网速占用」，但**不带任何外部依赖**
（不要求用户装 libpcap / nethogs / ethtool）。

三平台调研结论（对 crates/x-platform 现有数据源逐一核实）：

| 平台 | 每进程字节计数的第一方来源 | 无特权可用？ | 结论 |
|------|--------------------------|------------|------|
| Linux | 内核**不按进程**记流量；`/proc/net/*` 只有 socket 表（x 已读，含 inode→PID 归属），没有字节计数列；每进程计数需要 eBPF（特权）或 xt_account（iptables 模块，写内核状态） | eBPF 要 root | **不存在无特权来源** |
| macOS | `netstat -ib` / `getifaddrs` 只有**按网卡**累计（Ibytes/Obytes）；libproc、sysctl 均无 per-pid 网络计数；nettop 驱动的私有框架不给第三方 | — | **不存在任何来源** |
| Windows | ETW `Microsoft-Windows-Kernel-Network` 按 PID 记每包字节数，Rust 可零依赖直调 `StartTrace`/`EnableTraceEx2` | 需管理员 | **有，但要 admin** |

**诚实结论**：「无依赖 + 无特权 + 三平台」的每进程实时网速不存在。
本方案不假装它存在，而是给出**分层能力**：各平台取自己真实能取到的层级，
拿不到的如实标 unsupported（与 `x bluetooth` 同一原则）。

## 1.1 补充：macOS/Linux 也要每进程速率 —— 纯 Rust 抓包路线（需 root）

要求三平台都出每进程速率，唯一无外部依赖的路线是**自己抓包**：
内核的原始抓包接口（Linux `AF_PACKET`、macOS `/dev/bpf*`、Windows
`AF.NDIS` 或退回 ETW）都是 libc/系统调用，零第三方 crate 可直达。
代价与边界：

| 项 | 事实 |
|----|------|
| 特权 | 抓包 socket 需要 root（macOS 实测 `/dev/bpf*` 为 `root:wheel 0600`；Linux `AF_PACKET` SOCK_RAW 同样 CAP_NET_RAW）。**x 不提权运行时就拿不到**，`x net top` 在无 root 时 L3 如实缺席并给 hint |
| 依赖 | 无——libc 常驻，BPF/PACKET 的 ioctl 结构手写 FFI，与现有 port.rs 的 sysctl/netlink 路线同风格 |
| 归属 | 抓到的包只有五元组，归属进程靠**连接表快照**（已有）：TCP 有连接即归属；UDP 无连接，按「源端口 ∈ 某进程已绑定端口」归属，绑过又释放的端口会归错——这是该路线的固有误差，如实标注 `source: port-inference` |
| 开销 | 采样 socket + userspace 逐包计数。用 BPF 过滤器（macOS）/`SO_ATTACH_FILTER`(Linux) 在内核丢掉无关包，只收 IP 头做 (dir, bytes, 五元组) 统计，吞吐可控；仍建议默认 2s 窗口 + `--no-capture` 逃生舱 |
| 融合 | 与 ETW 同一个 L3 接口：`CaptureSampler` trait，Windows 实现 = ETW，Linux/macOS 实现 = BPF/PACKET 抓包。上层对拍、排序、渲染完全共用 |

这条路线让三平台的 L3 都存在，但**语义不同**：Windows ETW 是内核按 PID
报账（准）；Unix 抓包是「端口推断归属」（同端口先后易主时会错记）。
输出里保留 `source` 字段（`etw` / `port-inference`）让这个差别可见，
不把它抹平成同一等价物。

## 2. 能力分层

```
L3  每进程速率      Windows(admin): ETW 会话            ← 真·nethogs 等价物
L2  每连接归属性    三平台: 连接表快照对拍（已有数据源）  ← 谁在用什么端口说话
L1  整机速率        三平台: 网卡计数对拍（已有数据源）    ← 总量多少
```

命令 `x net top` 一次输出三层的并集，每行标注自己属于哪层、缺什么：

```
PID    PROCESS      RX/s      TX/s     CONNS  SOURCE
1234   chrome       1.2 MB/s  45 KB/s  18     per-process (ETW)
5678   ssh          -         -         2     connections only
-      (unmapped)   3.1 MB/s  120 KB/s  -     host counters only
```

- 某进程有 ETW 计数 → RX/s、TX/s 实数
- 只有连接表归属（Linux/macOS、或 Windows 无 admin）→ CONNS 有值，速率为 `-`
  （连接存在 ≠ 正在传数据，不猜速率）
- 网卡有流量但归不到进程（短命 UDP、内核态流量）→ unmapped 行

## 3. 快照对拍模型（复用 x/events 的既有模式）

与 `x events` 同一套机制，不发明新东西：

- `NetTopSampler` 每 `--interval`（默认 2.0s）采一轮快照；
- L1：`getifaddrs`(macOS) / `/proc/net/dev`+`ethtool` 慢路(Linux) / `MIB_IF_ROW2`(Windows)
  的 ifIndex→(rx_bytes, tx_bytes) 累计值，两轮差 ÷ 间隔 = 速率；
- L2：三平台连接表（`/proc/net/*`+inode 归属 / `libproc`+`libinfo` /
  `GetExtendedTcpTable`——全部是 x-platform 现有代码）对拍连接增删，
  新增连接记到其 PID 名下；
- L3（仅 Windows+admin）：ETW 内核网络事件的 (PID, bytes) 流水，按间隔聚合；
  Unix 侧此层整层缺席，不模拟。

计数器回绕（64 位 counter 理论上不回绕，但接口重建会清零）与「采样失败」
处理沿用 x/events 的规则：差值为负→丢弃该接口本轮并注记；
某层读失败→打 `! 层 采样失败: 原因`，本轮该层不参与对拍，恢复后重置基线，
**绝不把「读不到」渲染成「全没了」**。

## 4. CLI 形状

```
x net top [--interval 2.0] [--count N] [--pid PID] [--sort rx|tx|conns|pid]
          [--interface IF] [--json] [--jsonl]
```

- 退出码：0 成功；1 采样失败（无权限/无接口）；7 平台无任何一层可用；130 拒绝确认（若未来加写操作）。
  契约里的 6（采样超时）在本实现中不产生——抓包窗口由 `--interval` 决定，读不满时如实输出已有数据
- 破坏性？**否**——抓包只读、连接表只读，不写审计（与 `x logs` 同级）
- `--interface`：抓包设备（默认 macOS `en0`、Linux `any`）
- `--json`：每轮一个文档 `{ "interval_s":…, "host":{…}, "processes":[…] }`，
  缺失层整字段缺席（不输出 null 假装是 0）
- `--jsonl`：与 `--json` 同样的文档，每轮紧凑单行输出，供管道按行消费
- TUI：`x tui` 的第 9 页（数字键 9 / 命令面板 go to net top），后台线程采样
  （抓包窗口不阻塞绘制循环），无采样器接线或无权限时显示 hint 而非假数据

## 5. 平台降级矩阵（实现前的契约）

| 平台 | L3 每进程速率 | L2 每连接归属 | L1 整机速率 |
|------|-------------|-------------|------------|
| Linux | ✗ 如实缺席 | ✓（现有 port.rs 数据源） | ✓ |
| macOS | ✗ 如实缺席 | ✓（现有 network.rs 数据源） | ✓（netstat -ib 已在用） |
| Windows | ✗ 无可用来源，如实缺席并提示（ETW/ESTATS 路线已证伪：`SetPerTcpConnectionEStats` 需管理员且返回 `ERROR_ACCESS_DENIED`） | ✓（现有 GetExtendedTcpTable） | ✓（MIB_IF_ROW2 已在用） |

`x capability --domain net` 增加 `net-top` 条目，按上表报告 supported/degraded/unsupported。

## 6. 实现切片

1. **x-core**：`net_top.rs` 纯模型 + 对拍函数（快照结构、差值计算、排序），
   零平台知识，测试用固定快照对钉住（回绕、接口消失、基线重置）；
2. **x-platform**：各适配器暴露 `host_counters()` 与复用 `connections()`；
   Windows 加 `etw.rs`（StartTrace 直调，~200 行 FFI，单独 devlog 记录权限探测）；
3. **x-cli**：`net.rs` 加 `Top` 子命令 + 渲染（对齐 x/events 的轮次输出形状）；
4. **capability**：注册 net-top 探测项。

预估规模：x-core ~300 行 + 平台层 Linux/macOS ~100 行（全是拼装现有读数）、
Windows ETW ~300 行、CLI ~150 行。

## 7. 明确不做

- 不做抓包聚合（libpcap/BPF 依赖违背本方案前提）；
- 不在 macOS/Linux 上用「连接数 × 平均包大小」之类估算冒充速率；
- 不做 UDP 无连接流量的进程归属承诺（三平台都拿不到可靠归属，unmapped 行如实兜底）；
- ETW 会话崩溃残留由 `ControlTrace` 清理 + 启动时检测同名陈旧会话。
