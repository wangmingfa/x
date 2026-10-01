# 2026-10-01 · 内核级数据：`x port stats` 与 socket 队列长度

P3 的最后一项。新增 `x port stats`：按状态 / 协议统计 socket 数，并给出
内核队列占用。`PortInfo` 增加 `send_queue_bytes` / `recv_queue_bytes`
两个可选字段，三平台按各自内核实际提供的数据填空，不提供的留 None。

## 决策

**统计是纯归约，不另走采样。** `x-core` 的 `summarize(&[PortInfo])` 就是
对同一份快照做计数（`by_state`、`by_protocol`、`queues`），所以 stats 的
数字必然与 `x port all` 的行数对得上——不存在第二条采样路径与主列表
打架的可能。

**队列方向分开计数，缺数据不冒充 0。** `QueueStats` 里 send / recv 各自
有 `*_reporting` 计数与字节和：macOS 只有 TCP 发送侧（`libproc` 不暴露
接收队列占用），Windows 两侧都没有（IP helper 的任何 MIB 表都没有队列
字段），所以 CLI 只在某方向确实有 socket 报告时打印该行，两侧都没有时
打印 `queue lengths: not reported by this platform`，而不是给一串 0。
`backed_up` 只统计"报告了且非零"的 socket。

**Linux 原文透传。** `/proc/net/{tcp,udp}{,6}` 的 `tx_queue:rx_queue`
列（十六进制）直接进模型。监听 socket 上内核在这两个字段打印的是
accept 队列计数器而不是字节数——注释里写明"原样透传"，不在 x 里再
解释一遍语义，避免把平台的怪癖二次包装成谎话。

**macOS 只取发送侧。** `tcp_connection_info.tcpi_snd_sbbytes`（send
buffer 里的字节数）在 `tcpsi_tcbc` 里。偏移从已钉死的 `tcpi_state`
（`OFF_TCP_STATE`，其正确性由 macOS CI 上的活体监听测试保证——状态读出
Listen 才可能对）再走 4 字节头部 + 7 个 u32 得到，即 +32。只对 TCP 分支
读取：union 的其它分支同一偏移是别的字段，读出来的会是假数据。接收队列
libproc 不给，留 None。活体测试断言空闲监听 socket 发送队列恰为 0 且
接收为 None——若偏移错位，读到相邻的 `snd_wnd`/`rcv_wnd` 等非零字段会
在 macOS CI 上立刻失败。

**Windows 如实没有。** `GetExtendedTcpTable` / `GetExtendedUdpTable` 的
MIB 结构里没有队列占用；不给列、不猜。活体测试断言两个字段都是 None，
把这个"平台的缺席"钉住，防止将来有人好心填 0。

**TUI 详情顺带展示。** socket 详情对话框在报告了队列长度时多两行
（没有就整行不出现），与 CLI 的诚实口径一致。

## 验证

- 单测新增 10：x-core `summarize` 4（状态 / 协议计数、方向级 reporting、
  空快照、JSON 键名）、Linux 解析 1（非零 `tx:rx` 十六进制、逐字段降级）、
  CLI 2（stats 表格 + JSON 数字、队列未报告时的措辞）、TUI 渲染 1
  （详情里的 send/recv 队列行）、Windows 活体 1（队列字段为 None）、
  macOS 活体 1（空闲监听发送队列 0、接收 None）。
- 本机活体（Windows）：`x port stats` → listen 23 / established 35 /
  time_wait 21 / bound 20 / unknown 3 / total 102，tcp 82 / udp 20，
  `queue lengths: not reported by this platform`，退出码 0；`--json`
  与表格同源；`--proto tcp --state listen` 过滤后统计为 23。
- Linux 口径核对：WSL 里真实 `/proc/net/tcp{,6}`、`/proc/net/udp` 的
  列形状与解析样本一致（`sl local_address rem_address st
  tx_queue:rx_queue tr tm->when ...`，监听行为 `00000000:00000000`）；
  Linux 解析测试在 ubuntu CI 上跑真机。
