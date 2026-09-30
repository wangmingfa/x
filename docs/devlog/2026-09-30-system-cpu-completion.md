# 2026-09-30 P0 数据补全：系统信息与 CPU（三平台）

本批完成路线图 P0 的「系统信息」与「CPU」两项（commit `0a74fb9`），
macOS 实机验证，Linux/Windows 走 `cargo check --target` + CI。

## sysinfo 0.39 的隐藏行为

- **`Cpu::max_frequency()` 在 0.39 里没有了**。最大频率必须自己找平台 API：
  macOS 读 `hw.cpufrequency_max`（仅 Intel）、Linux 读
  `/sys/devices/system/cpu/cpufreq/policy*/cpuinfo_max_freq`、Windows 用
  `CallNtPowerInformation(ProcessorInformation)` 的 `MaxMhz`。
- **`refresh_cpu_usage()` 不刷新频率**。要拿每核频率必须
  `CpuRefreshKind::nothing().with_cpu_usage().with_frequency()` 再走
  `refresh_cpu_specifics`，否则 `Cpu::frequency()` 恒为 0。
- **单位三处三个样**：sysinfo 的 `frequency()` 是 MHz；macOS `sysctl hw.cpufrequency*`
  是 Hz（要除 1e6）；Linux `cpuinfo_max_freq` 是 kHz（除 1000）；Linux
  thermal/hwmon 的 `temp` 是毫摄氏度（除 1000）。对不上就是数量级错了。
- **Windows 没有真 load average**。sysinfo 用 PDH 采样 `\System\Cpu Queue Length`
  做 EMA 折算成 1/5/15 分钟，采样周期 5 秒，**头几秒读到的是 0**，不要当成 bug。
- **Apple Silicon 没有 `hw.cpufrequency_max`**，sysinfo 的当前频率是走 IOKit
  `AppleARMIODevice` 拿的（参考 psutil 的做法），M4 Max 实测 4512 MHz。
- **温度不要拿 `acpitz` 充数**。它是主板热敏电阻不是 CPU；只认
  `coretemp/k10temp/zenpower/cpu_thermal/soc_thermal` 或名字带 `cpu/pkg` 的传感器。
  另外 `temp` 读到 `0` 表示驱动还没刷过值，必须丢弃（见
  `crates/x-platform/src/linux/system.rs` 的 `milli_to_celsius`）。

## Windows API 的坑

- **`GetLogicalProcessorInformationEx` 的记录头**：前 4 字节是 `Relationship`，
  后 4 字节才是 `Size`。旧代码把第一个 u32 当长度，而 `RelationProcessorCore == 0`，
  长度为 0 直接 break —— Ex 路径**永远失败并静默回退旧 API**（>64 逻辑核的机器
  会数错）。修复后的遍历在 `windows/system.rs` 的 `processor_records`，单测用
  构造 buffer 验证「Relationship 为 0 不中断」。教训：遍历变长记录前，
  先核对 SDK 头文件里每个字段的偏移，别想当然。
- **`EfficiencyClass` 的方向**：0 = 性能最高的核，数值越大越省电（E 核），
  见 `windows/system.rs` 的 `split_by_efficiency_classes`。P/E 按 record 数
  （一个 record 一个物理核）统计。
- **windows-sys 0.61 的函数指针是强类型的**：`AlignedBuffer::as_mut_ptr()`
  返回 `*mut c_void`，传给 `GetLogicalProcessorInformationEx` 会 E0308，
  必须 `.cast()` 成具体类型。
- **函数都在哪**：`GetLocalTime`/`GetSystemTime` 在 `Win32::System::SystemInformation`
  而不是 `Win32::System::Time`（那边只有 `SystemTimeToFileTime`）。
  grep 不到就先怀疑模块归属。

## Linux 的坑

- **`/proc/cpuinfo` 的 topology 不保证存在**：ARM 和多数 VM 没有 `physical id` /
  `core id`。P/E 推断必须容忍空 topology（每个 CPU 自成一核兜底），见
  `linux/system.rs` 的 `split_clusters`。
- cpufreq 的 `policy*` 目录里，`related_cpus` 是时钟域（含离线核），
  目录名 `policyN` 是兜底的 CPU 号来源。

## 架构约束带来的写法

- `x-core` 有架构测试禁止 `cfg(target_os)`。平台相关的**纯逻辑**要写成
  「数据注入参数」的函数（如 `split_clusters(clusters, &topology)`），
  这样不碰 `/proc`、`/sys` 就能在任意平台单测。
- Linux/Windows 模块在 macOS 上根本不编译，它们的单测只在 CI 跑。
  想立刻验证纯函数：把函数和断言拷进临时 `main.rs`，用
  `rustc -O --edition 2021 main.rs` 独立编译运行（注意不加 `--edition`
  默认 2015，`try_into` 不在 prelude）。

## Rust 杂项

- `serde_json::Value` 比较 `assert_eq!(v["x"], 1.2.into())` 会 E0283（`into` 歧义），
  用 `v["x"].as_f64()` / `.as_u64()` / `.as_i64()` 比较。
- `to_string_lossy()` 返回临时 `Cow`，不能直接链 `.strip_prefix()` 再返回引用
  （E0515），先 `let text = ....into_owned();` 绑定。
- `Option<String>.and_then(f)` 里 f 收 `String`，要 `&str` 就写闭包
  `.and_then(|raw| parse(&raw))`。

## 验证方法存档

- load average 对拍：`sysctl -n vm.loadavg` / `uptime` 与 `x sys cpu` 输出一致
  （10.4x / 6.5x / 5.4x）。
- P/E 对拍：`sysctl -n hw.perflevel0.physicalcpu`（=10）、
  `hw.perflevel1.physicalcpu`（=4）与 `x sys info` 的 performance/efficiency
  cores 一致。
- 重启时间对拍：`sysctl -n kern.boottime` 的 epoch 经 `date -r <epoch>` 与
  `x sys info` 的 last reboot 一致。
