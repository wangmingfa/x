# 2026-10-01 P0 数据补全：端口远程地址 / port find / port watch（三平台）

本批完成路线图 P0 的「端口」项：socket 表增加远程地址列、
`x port find <name>` 按进程名反查、`x port watch` 轮询差异输出。
本机（Windows）实测通过；Linux/macOS 走 `cargo check --target` + CI。

验证过程中连带修掉一批 Windows 适配器的存量缺陷，一并记录。

## AlignedBuffer::read_at 的单位坑（全 Win32 表遍历都会 AV）

`read_at::<T>(start)` 的 `start` 语义是**字节偏移**，旧实现却写成
`.cast::<T>().add(start)`——指针加法按元素走，TCP/UDP 表、服务枚举、
`SYSTEM_LOGICAL_PROCESSOR_INFORMATION`、`PROCESSOR_POWER_INFORMATION`
所有多行表在第 2 行起就读到越界内存，`x port all` 直接 segfault。
修复：先 `cast::<u8>().add(start)` 再 `cast::<T>()`，配合
`read_unaligned`（Win32 变长表不保证行对齐）。
证据：`crates/x-platform/src/windows/buffer.rs` read_at；修复前
`./target/debug/x.exe port all` 退出码 139。

## EnumServicesStatusExW 不接受 dwServiceState=0

现象：`x service list` 恒返回空 / win32 error 87。
排查：参数扫描显示 `(SC_ENUM_PROCESS_INFO, SERVICE_WIN32, 0)` → 87
ERROR_INVALID_PARAMETER，把 state 换成 1（SERVICE_ACTIVE）立刻
`ok=1 returned=98`。根因：这个参数和别的 Enum* 不同，**0 不是
「所有状态」**，必须显式传 `SERVICE_STATE_ALL`（= ACTIVE|INACTIVE = 3）。
另外它也不支持 NULL 缓冲的 size-query（needed 恒 0），只能按
resume handle 分块循环。
证据：`crates/x-platform/src/windows/service.rs` list()；
`x service list` 现在返回全量服务。

## LookupAccountSidW 的 peUse 不能传 NULL

旧实现把最后一个参数 `peUse` 写成 `&mut 0` 的临时量（UB）/NULL，
成功路径在 advapi32 内部 AV。正确姿势：尺寸查询和填充查询都必须传
真实的 `SID_NAME_USE` 变量，且填充调用时 name 和 domain **两个缓冲
区都要真实存在**（旧代码 domain 传 NULL + 非零长度，API 直接失败）。
证据：`crates/x-platform/src/windows/mod.rs` lookup_sid；
修复后 port 表 user 列能显示 `WMF\wmf12`。

## Windows TCP 表：监听行的 remote 是 0.0.0.0:0，不是「无对端」

`MIB_TCPROW_OWNER_PID` 对 LISTEN 行填零而不是留空，直接展示会污染
remote 列。与 Linux 适配器的既有约定对齐：remote 端口为 0 ⇒ 视为
None。v4/v6 都处理。
证据：`x port all --plain` 中 `0.0.0.0:135` 行 remote 列为 `-`。

## GetExtendedTcpTable 的端口是网络序、藏在 u32 低 16 位

`(raw as u16).to_be()` 是对的；单元测试夹具原先写成 0x1F90 才错——
8080 的线序字节 1F 90 放在小端 u32 里是 0x0000_901F。改夹具不改实现。

## watch 的「只在变化时输出」放在 x-core 纯函数里

x-core 的原则是 Snapshot, never watch，但**差异比较**本身是平台无关
的：`SocketKey`（proto/local/remote/state/pid）+ `diff_sockets`，
CLI 只做轮询与呈现。状态变化会呈现为「一行删 + 一行增」，与 ss 的
观感一致，也让 JSON 输出保持 added/removed 两个数组。
证据：`crates/x-core/src/port/model.rs`；
`x port watch --interval 0.05 --count 2` 稳定网络下只打印基线行。

## 工具坑：改了 x-platform 别忘了重编 x-app

`x.exe` 由 `x-app` 产出，`cargo build -p x-cli` 不会重链它；cargo 的
mtime 判定还可能让 `x.exe` 带着旧适配器「看似最新」。服务枚举修好后
CLI 仍报 87 就是这个原因。验证行为一律 `cargo build -p x-app` 后再跑。
