# 2026-09-30 P0 数据补全：内存（三平台）

本批完成路线图 P0 的「内存」项：`MemoryUsage` 增加
`swap_total_bytes` / `swap_used_bytes` / `pressure`，macOS 实机验证，
Linux/Windows 走 `cargo check --target` + CI。

## 数据模型

- 压力等级统一为 `PressureLevel`（normal / warning / critical），
  放在 `x-core`；`None` 表示平台不发布该信号。
- Swap 语义按平台取本义：macOS/Linux 是 swap，Windows 是 page file
  （sysinfo 的 `total_swap()`/`used_swap()` 恰好就是 commit charge）。
- `0` swap 表示「无或不可知」，CLI/TUI 对 0 直接隐藏该行。

## macOS 的坑：`vm.swapusage` 是二进制不是字符串

`sysctlbyname("vm.swapusage")` 返回 32 字节的 `struct swapusage`
（total/used/free 三个小端 `u64` 字节数 + flags），`sysctl(8)` 里那行
`total = 14336.00M ...` 是 CLI 自己格式化的。第一版按字符串解析，
`sysctl_string` 拿到非 UTF-8 返回 `None`，swap 静默变成 0 ——
实机对拍才发现。教训同 CPU 批次：**先看 sysctl 的原始字节，
别信 CLI 的输出格式**。

压力等级读 `kern.memorystatus_vm_pressure_level`（1..=4，
1=normal、2=warning、>=3 按 critical 处理）。

## Linux / Windows

- Linux：swap 直接用 sysinfo（`/proc/meminfo` 的 SwapTotal/SwapFree）；
  压力读 `/proc/pressure/memory` 的 PSI `full avg10`
  （>=5% warning、>=25% critical），`CONFIG_PSI=n` 或容器里文件缺失时
  返回 `None` 而不是硬猜 normal。
- Windows：没有单一代码点能读出「压力等级」，用物理内存负载百分比
  代替（>=90 warning、>=95 critical，和任务管理器配色同一信号）；
  阈值映射写成纯函数 `pressure_from_load` 可单测。
