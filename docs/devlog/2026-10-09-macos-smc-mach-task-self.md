# 2026-10-09 · macOS SMC 温度：一个数据符号、一套类型口径、一组不是温度的回答

真机（Apple silicon）上 CPU 温度这条链路一跑就死，修好之后又暴露出两个更微妙的问题：
Apple silicon 的传感器压根不用 `sp78`，以及某些采样里 SMC 整组回答的不是温度。CI 三平台
全绿、本机 `cargo test` 也绿——因为守护它的那几行在 VM 上从来没被执行过。

## 现象

- `x sys cpu`、`x sys watch --count 1`、`x capability` 无任何输出直接退出 138
  （128 + 信号 10 = SIGBUS）。
- TUI Dashboard 的 `refresh_dashboard`（`crates/x-tui/src/app.rs:586`）走同一个
  `cpu_usage()` 入口，在同一台机器上一起炸；`x sys info` / `x sys mem` 不受影响，
  因为 SMC 只在 CPU 采样里读。
- GitHub macOS runner 上这三条命令都不崩。

## 根因

`mach_task_self_` 是 libsystem_kernel 导出的**变量**，`mach/mach_init.h:80` 写得很清楚：
`extern mach_port_t mach_task_self_;`——C 里的 `mach_task_self()` 只是读它的宏。
`crates/x-platform/src/macos/smc.rs` 原先把它声明成 `fn mach_task_self_() -> MachPort`
并调用。`extern "C"` 块不查符号种类，链接照过，运行时 PC 直接落进 `__DATA`：
把 task port 的数值当指令执行。

这类错误的共同点是**不返回错误码**，和同文件已经记过的 `size_t` 宽度是同一族：
错了不是「调用失败」，是内存/控制流被改掉。

## 证据

- lldb：`stop reason = EXC_BAD_ACCESS (code=2, address=0x209ac0da0)`，
  `frame #0: libsystem_kernel.dylib\`mach_task_self_`，反汇编首条 `udf #0x203`。
  那 4 个字节就是本进程的 task port `0x203`——数据被当成了代码。
- 独立对照（C，不经 x 的代码路径）：`printf("0x%x", mach_task_self())` 打出 `0x203`；
  把同一符号在 C 里再按函数声明一次，clang 立刻报
  `redefinition of 'mach_task_self_' as different kind of symbol` 并指回头文件那一行。
- 测试侧红/绿：修前 `cargo test -p x-platform --lib smc` 里 9 个 `common::smc` 测试
  通过，第 10 个 `macos::smc::tests::the_read_degrades_without_panicking` 把测试进程
  打死（`signal: 10, SIGBUS: access to undefined memory`）；修后 10/10 通过，0.25s。
- 修后正向验证（lldb 断点，`smc.rs:93`）：`IOServiceOpen` 的 `x1 = 0x203`（真实 task
  port），返回 `x0 = 0`（KERN_SUCCESS）。传参对了，不是「恰好提前返回」。

## 处理

声明成变量、在既有 unsafe 调用里按值读（`smc.rs:60-63` 声明，`:93` 调用），模块头的
ABI 事实清单补了这一条。

## 值得留档的坑：VM 上的绿不等于路径跑过

`Smc::open()` 在 `IOServiceGetMatchingService` 返回 0 时就 `return None`
（`smc.rs:86-89`）。CI 的 macOS runner 是虚拟机，没有 `AppleSMC` 服务，于是
`IOServiceOpen` 那一行在 runner 上**从未执行**，「读不出也不崩」的断言空跑一遍就过了。
这条断言只在真有 SMC 的机器上才算数；最省事的确认是看它的耗时——本机 0.25s 意味着
它真的枚举了 2109 个键，而不是走到第一个 early return。

## 第二个坑：Apple silicon 的 die 传感器根本不是 `sp78`

修完 ABI 不崩了，温度仍然是空的——既不是权限也不是虚拟机没有键。

**根因**：C 探针逐个键打印 `keyInfo`，本机 45 个 `Tp0*` 的类型清一色 `flt `
（小端 IEEE-754 单精度），`sp78` 键**一个都没有**；旧代码只接受 `DATA_TYPE_SP78`，
于是所有传感器都被跳过。`sp78` 口径只在（部分）Intel 机器上成立。

**处理**：`common/smc.rs:189` 的 `decode_temperature(data_type, bytes)` 按声明类型分流，
`sp78` 走大端 8.8、`flt ` 走小端单精度（`common/smc.rs:178`），认不出的类型不解码；
−40…150 ℃ 的窗口顺带挡掉 NaN 与无穷——它们对任何比较都为假。macOS 侧只剩一次调用
（`macos/smc.rs:232`）。

**证据**：

- 夹具是真实载荷，期望值按 IEEE-754 定义算：`00 20 83 42` = 65.5625、
  `00 f8 99 42` = 76.984375、`00 00 c0 41` = 24.0。不是「解码器自己跟自己一致」。
- 反例钉住类型的作用：同一串 `00 20 83 42` 按 `sp78` 解是 **0.125 ℃**，同样落在窗口内。
  选错解码器得到的是一个看不出破绽的错数，所以 `the_declared_type_picks_the_decoder`
  把两种解释都钉住，并断言 `ioft` / `ui8 ` / 0 一律 `None`。
- 与独立实现同时刻对拍（C 直接走 IOKit，不经 x 的代码）：C `hottest=Tp06 76.53` 对
  CLI `75.84375`；隔一秒再对，C `75.61` 对 CLI `75.6`。两侧 `matched` 都是 45。

## 第三个坑（未处理）：有些回答不是温度

**现象**：`x sys watch` 的个别样本明显不对——同一进程里 `73C`、`5C`、`73C` 交替出现，
`--json` 里是 `5.1999998`、`40.0`。

**证据**（每轮 open→枚举→取值→close 的独立 C 探针）：

```
pass 0 (fresh client): matched=45 keysBelow20=45 max=5.20
pass 1 (fresh client): matched=45 keysBelow20=45 max=5.20
pass 2 (fresh client): matched=45 keysBelow20=0  max=75.73
pass 3 (fresh client): matched=45 keysBelow20=0  max=75.73
--- same client reused, no re-open ---
pass 0..2 (shared)   : keysBelow20=0  max=75.16 / 75.16 / 74.00
```

占位组不是噪声：45 个键整齐地落在 {−4.0, 0.0, 2.5, 4.0, 5.2} 上，并按键号每 4 个一组
重复——像是整张表被同一个未分化的状态回答，而不是某个传感器读错。Rust 侧用一个临时
测试（跑完即删）复现同一现象：前两轮 max=5.2 且 45/45 个键低于 20 ℃，第三轮整组 40.0。

**为什么先不修**：这不是补一个分支能了事的，选择哪种防护是设计决策——每个样本多读一轮
（`x sys cpu` 要多花约 0.25s，且「第二轮可信」这件事本身还没排除是时间窗口的巧合）、
还是进程内复用同一个 user client（watch / TUI 受益，one-shot 不变）、还是给「整组未分化」
这种回答一个如实的「它还没说」判定；三者的取舍需要在更多机型上取证，不该由一台机器的
观察替全线定调。**当前可见后果**：one-shot 在本机多数时候给出真读数，但同一套代码在
watch 的个别样本和测试的首轮读到过占位值——机制（每个 client 的前几轮 / 时间窗 / 系统级
缓存被频繁读打断）没有定论。

