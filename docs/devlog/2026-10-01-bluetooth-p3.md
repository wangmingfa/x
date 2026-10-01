# 2026-10-01 · P3 蓝牙域（三平台）

新增 `x bluetooth adapters | devices | scan | connect | disconnect`。
核心模块 `x-core/src/bluetooth.rs`，适配器 `x-platform/src/common/
bluetooth_os.rs`，命令 `x-cli/src/commands/bluetooth.rs`，审计装饰器
`AuditedBluetooth`。

## 决策

**能力不对等写进 trait。** `scan/connect/disconnect` 在
`BluetoothManager` 上是带默认实现的方法，默认返回 Unsupported。
Linux（bluetoothctl）覆写全部动词；Windows / macOS 只实现读，动词
自然落到默认「不能不碰 GUI 就连接」的诚实错误——命令在每个平台都
存在，退出码 7 如实说明为什么不行，不做假成功。

**地址在校验点规范，不在平台点补救。** `normalize_address` 要求六段
冒号分组的十六进制，统一大写；CLI 动词第一步就调用它，`bluetoothctl
connect garbage` 这类"把垃圾递给别人的词汇表"的失败模式在入口终结。
被拒的输入（退出码 5）不触发确认、不写审计——还没到"要动机器"的
那一步。

**Windows 按实例路径命名空间切分。** `Get-PnpDevice -Class Bluetooth`
里 `BTHENUM\*` 是经由无线电枚举出来的远端设备，其余是本地对象；
设备地址从实例路径中的 12 位十六进制段还原成 `80:A9:CD:54:6B:81`，
`LOCALMFG` 占位实例提不出地址就如实缺席。局限：TDI/配置文件行
（`蓝牙 LE 通用属性服务` 等）会落进 adapters——命名空间启发式只能
做到这么粗，但每行都是 PnP 原话，状态列（`OK`/`Error`）与中文名
原样透传。

**Linux 降级要降得干净。** `devices Paired` / `devices Connected` 是
较新 bluetoothctl 才有的过滤参数，旧版直接报错：解析器捕获失败后把
对应标志留 `None`——"平台没说"不等于"否"，表格里显示 `-`。动词另
有一个坑：bluetoothctl 非交互模式下失败照样打印 `Failed: …` 并以
退出码 0 收场，只看 status 会把失败报成成功，所以额外扫描 stdout
的 `Failed:` 行，命中则报 InvalidState。

**macOS 的 device_list 有两种布局**（新版数组套单键对象、旧版直接
映射），walk 两种形状统一吐 `{name: fields}`；`connected/paired`
只有 `Yes/No` 才映射布尔，`minority` 这类值不认识就缺席。RSSI 形如
`-54 (0xca)`，取行首数字。

**审计装饰器与 firewall 同规格。** `AuditedBluetooth` 只包
`connect/disconnect`（`bluetooth.connect` / `bluetooth.disconnect`，
target 是规范化地址），reads 直通；attach() 里新增字段必须逐字段
重建 SystemContext，蓝牙按此模式挂载。被平台拒绝的尝试同样落盘。

## 验证

- 单测：core 3（规范化、坏形状、JSON 缺席）、platform 5（PnP 三形态、
  BTHENUM 切分与地址提取、controller/powered/devices 解析、macOS 双
  布局、RSSI）、audit 1（读不留痕 / 动词留痕）、CLI 7（表、JSON、
  规范化记录、坏地址 5、确认拒绝不动机器、scan 默认 7、无能力 7）。
- 交叉 clippy 当晚跑全三个 target（Linux/macOS 分支一次通过，得益于
  import cfg 门控与 `|e| e.ok()` 的旧教训）。
- 本机活体：`x bluetooth adapters` 12 行（中文枚举器名、`Error` 状态
  原样）；`devices` 空 → 「no bluetooth devices known to the platform」
  退出 0；`connect aa:…:ff -y` → 退出 7，audit.log 追加
  `bluetooth.connect … outcome:"error: this platform cannot connect
  Bluetooth without a GUI"`。
