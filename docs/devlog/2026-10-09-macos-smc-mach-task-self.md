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

## 第三个坑：有些回答不是温度（已取证，防护已按内容实现）

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

**取证方式（2026-10-09 补，随机交替代替换机器）**：只有一台 Mac，换机取证不可行，但混淆
可以在本机拆开——独立 C 探针（不经 x 的代码，`sizeof(SMCData)==80`、`keys=2109`、
`cpu_keys=45`），五种条件按随机顺序抽样、步间随机延时：

- `A1` 新开 client + 全量枚举 + 读（等于 x 今天走的路径）
- `A2` 同一个 client 内再读一遍（不重新枚举）
- `B`  进程级复用的长命 client + 全量枚举 + 读
- `C`  新开 client、跳过枚举、只读已知键
- `D`  长命 client 空闲 12–16 秒后再读

两轮共 147 个样本：稀疏轮 65 行（48 步，步间 0–1.5s，含 5 次 D），密集轮 82 行
（60 步，步间 0–0.4s，不含 D）。判据记的是每次读出的**不同值个数**（distinct）、
低于 20℃ 的键数、以及整个值集合。

**结论 1：占位状态跟着时间走，不跟着 client 走。** 每个污染窗口里所有条件同时中。密集轮
t=0..4s 有 30 个连续样本（B/C/A1/A2 都有，新开的和复用的都有）是「45 个键同一个 40.000」；
t=4..5s 整组换成 {−4.0, 0.0, 2.5, 4.0, 5.2}；t=6s 之后整轮都是 44–45 个不同值。稀疏轮
同样在 t=0、16、29、92 秒各出现一次窗口，每次波及当刻的全部条件。多样本的秒里，
条件之间一致：稀疏轮 16 个可比秒中 15 秒一致，密集轮 12 秒中 10 秒一致（不一致的两秒
正是窗口换形状的过渡瞬间）。

**结论 2：候选 ①（每样本多读一轮）作废。** A1 与 A2 两轮 37 次配对里 36 次同判——同一个
client 内第二次读不比第一次好，多花的那 0.25s 买不到任何东西。唯一一次不一致是 t=0 的
`A1 distinct=14 → A2 distinct=1`：窗口正在换形状，而不是「读第二轮就读好了」。

**结论 3：候选 ②（复用长命 client）作废。** B 在窗口里同样中招（稀疏轮 18 次里 3 次、
密集轮 21 次里 8 次）。D 反而 5 次里 4 次读到真值，所以「空闲一段时间缓存过期」也不成立。

**只剩候选 ③，已实现**：按回答的**内容**判定，而不是按采样时机。区分度实测很宽——污染样本
distinct ≤ 6（1 个值或 5 个值），真实样本 distinct 44/45；窗口切换瞬间的过渡行落在中间
（7、8、14、16），而那些行本身就是「一部分键还没分化」的混合回答，一起拒掉并不亏。

落点是 `common/smc.rs` 的纯函数 `hottest_when_differentiated(&[f32])`：规则取
`distinct >= ceil(matched / 2)`，不满足就 `None`。四个数对着上面的实测边界——45 键要 23 个
不同值，占位的 1/5/6 全被挡，真实的 44/45 全过；只有一个键的机器（Intel 常见）
`ceil(1/2)=1`，永不误伤；两个键同值也照给，因为单靠两组值判断不出「塌缩」，宁可少判。
macOS 侧读循环改成先收齐全组再交给它：**边读边折最大值看不出塌缩**，一个孤立的热核单独看
永远合理。放在 common 的额外好处是这段判断逻辑三平台都编译、都在 CI 里真跑，不像 IOKit
那层只能本机活体。Linux 的 `thermal_zone`/`hwmon` 路径**不动**：那是另一套来源、另一种失效
方式，把 SMC 的行为规律套过去就是假统一。

**活体**（本机，`x sys watch --interval 0.2 --count 30`）：30 个样本里 21 个给温度
（72.4–81.3℃），9 个整段不出现 `temp` 字段——正是窗口；不再有任何 `5.2` / 平坦 `40.0` 的
样本。同一命令在加防护前会打出 `temp 5.2 C` 夹在真读数中间。窗口内的表现从「一个像真的
错数」变成「不说」，按现有口径渲染为「本平台未暴露」。

**仍未定论的是机制**：本批数据排除了「每个 client 的前几轮」和「空闲后缓存过期」两种解释，
剩下的形状是持续数秒的全局窗口，与 EC/固件刷新 sensor 表的节律一致，但没有文档支持这一步
推论，所以防护按内容判而不是试图按时间猜。

