# 2026-10-01 P0 数据补全：网络链路速度/网关、connections、DNS 操作、ping/trace/resolve（三平台）

本批完成路线图 P0 的「网络」项与「addresses DHCP 标记」项：
网卡表增加链路速度与默认网关列；`x net connections`（按进程/端口过滤）；
`x net resolve / reverse / flush`；`x net ping / trace` 走原生 ICMP。
Windows 路由表从 `route print` 文本解析换成 `GetIpForwardTable2`。
本机（Windows，zh-CN）实测通过；Linux/macOS 走 `cargo check --tests --target`。

## route print 在中文系统上不可解析（旧实现的隐性失效）

现象：`x net routes` 在本机长期只回很少的行甚至空。
根因：旧 Level-3 路径按英文列名/列序解析 `route print`，zh-CN 输出
表头、分隔线、网络名全变，解析器在第一个非常规行就停表。
处理：Windows 路由改走 `GetIpForwardTable2`（AF_INET + AF_INET6 各一次），
行内 `InterfaceIndex` 用 `GetAdaptersAddresses` 的索引→显示名映射翻译，
保证与 `x net interfaces` 用同一个字符串 join。
证据：修复后本机输出含 `以太网`、`蓝牙网络连接` 等中文接口名、metric 列
完整；`crates/x-platform/src/windows/network.rs` forward_table。

## MIB_IPFORWARD_TABLE2.Table 是占位数组，不是整表

`header.Table` 的类型是 `[MIB_IPFORWARD_ROW2; 1]`（C 柔性数组的投影习惯），
`header.Table.iter().take(NumEntries)` 只会拿到第 1 行，剩余行是越界读。
正确写法：以 `&raw const header.Table` 为基址做
`slice::from_raw_parts(…, NumEntries)`。用完 `FreeMibTable` 释放整块。
证据：首版实现只回一行路由；改成 raw slice 后行数与 `route print` 一致。

## windows-sys 0.61 没投影的三样东西

- `DnsFlushResolverCache`（dnsapi）：手工 `#[link(name = "dnsapi")]
  extern "system" { fn DnsFlushResolverCache() -> u32; }`。
- `MAKEWORD`：不是导出函数，直接写 `0x0202` 给 `WSAStartup`。
- `in_addr`/`in6_addr`：0.61 里叫 `IN_ADDR`/`IN6_ADDR`，但代码里其实
  不需要类型名，删掉导入即可。
另外 `Icmp6SendEcho2` 的参数含 `Win32_System_IO::PIO_APC_ROUTINE`，
Cargo.toml 必须加 `Win32_System_IO` feature，否则符号整个不可见。

## DnsFlushResolverCache 未提权返回 1，而不是 5

现象：普通 token 下 flush 返回 rc=1（ERROR_INVALID_FUNCTION）。
根因：dnsapi 把「服务拒绝非提升调用方」折叠成了 INVALID_FUNCTION，
文档只说「失败返回 FALSE」。处理：rc∈{1,5} 一律映射为
`PermissionRequirement::Administrator`，退出码 4；其余失败保持系统错误。
证据：`x net flush` → `error: flushing the DNS cache requires an elevated
token` + hint + exit=4。

## NDIS 用 u64::MAX 表示「没有介质」，不是速度

现象：`x net interfaces` 给断开的有线网卡显示 `18446744073.7 Gbps`。
根因：`IP_ADAPTER_ADDRESSES.ReceiveLinkSpeed` 对无介质/未协商驱动写
`0xFFFFFFFFFFFFFFFF`（有时 `-1`），对循环接口写 1.1 Gbps。
处理：`Adapter::known_link_speed` 把 0、`u64::MAX`、`u64::MAX-1` 归为
None；Wi-Fi 实测 216 Mbps（协商值），蓝牙 PAN 3 Mbps，均保留。
证据：修复后同一张表有线网卡显示 `-`。

## DHCP 位是「适配器启用 DHCP」，不是「这个地址来自 DHCP」

现象：断线的有线网卡把自己的 169.254.x（APIPA）也标成 yes。
根因：`IP_ADAPTER_ADDRESSES.Flags` bit 0x1 是适配器配置项，与当前地址
怎么来的无关；IPv6 链路与 SLAAC 更与 DHCPv4 无关。
处理：`address_rows` 只对「IPv4 且非链路本地且适配器 Up」的地址给
Some(bit)，其余 None（不知道 ≠ 不是）。macOS 用 `ipconfig getpacket <if>`
输出含 `yiaddr` 判定，IPv6 与 lo* 跳过；Linux 内核不记录地址来源，
诚实留空。
证据：本机 `x net addresses` 现在只有 WLAN 的 192.168.3.64 是 yes。

## Icmp6SendEcho2 坚持要源地址：用 connected UDP 问内核

IPv6 echo 的 API 签名要求真实源 `SOCKADDR_IN6`，传零地址直接失败。
做法：`UdpSocket::bind("[::]:0")` + `connect(目标)`（UDP connect 不发包，
只是让内核选路），读 `local_addr()` 得到与 `ping -6` 相同的源，再喂给
ICMP API。v4 侧不需要这一步，`IcmpSendEcho` 自己会选。
证据：`crates/x-platform/src/windows/netprobe.rs` ping_v6。

## ICMP API 的 reply 记录同时承载「目的应答」和「中间路由报错」

`IcmpSendEcho` 的 `ICMP_ECHO_REPLY` 记录里 `Status` 与 `Address` 合起来
才是完整事实：Status=0 → Address 是目的；Status=11010
（IP_REQ_TIMED_OUT）→ 没人应答（星号）；其他状态码（含 11041
IP_TIME_EXCEEDED）→ Address 是报错的路由器。trace 的 TTL 循环因此一个
API 就能做，不需要 raw socket。`IcmpCreateFile` 的句柄必须用
`IcmpCloseHandle` 关，用 `CloseHandle` 是错的。
证据：`x net trace baidu.com --max-hops 12` 显示网关 2 ms、运营商 NAT
100.80.x 4 ms、其余星号，与 tracert 走向一致。

## unix 侧用 SOCK_DGRAM + IPPROTO_ICMP，不碰 raw

Linux 的 unprivileged ICMP 走 `ping_group_range`，macOS 对 dgram-icmp
可能仍需 root。选 dgram 的理由：内核改写 identifier 并做校验和 demux，
不会串扰别的进程的 ping。EACCES/EPERM 明确报成 Root 提权提示而不是
悄悄回退 spawn `ping`。`NI_NAMEREQD` 必须有——否则 getnameinfo 查不中也
回数字形式，调用方分不清命中 PTR 还是没命中。
未上机部分以交叉编译 + 纯函数单测覆盖（echo_message/icmp_checksum/
classify_icmp/sockaddr 往返）。

## interfaces 的 default 列以前把所有有路由的接口都标 yes

`defaults` 收集的是全部路由行的接口名，不是默认路由的。既然网关列
已经按 `is_default_route` 过滤，default 列同样收敛。
证据：修复后只有 WLAN 是 yes，Loopback 不再误标。

## 解析失败要分级：NXDOMAIN 是 not-found，不是系统错误

`getaddrinfo` 的 WSAHOST_NOT_FOUND(11001)/WSANO_DATA(11004) 与
POSIX 的 EAI_NONAME/EAI_NODATA 表示「查无此名」，脚本要靠退出码 3
区分它和 DNS 挂了（保持 1）。
证据：`x net resolve nope.invalid` → exit=3。
