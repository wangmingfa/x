# 2026-10-01 P0 数据补全：能力探测 x capability（三平台）

本批完成路线图 P0 的「能力探测」项：`x-core` 新增 `capability` 模块，
`probe(context)` 用真实适配器逐项实测并归类
supported / degraded / unsupported；CLI 新增 `x capability`
（`--domain` 过滤，JSON 数组）。本机（Windows，zh-CN）34 行全量
实测通过；Linux/macOS 走 `cargo check --all-targets --target`。

## 用"故意非法的名字"探测特性是否存在，不执行任何东西

需求：能力报告要区分「logs/native 这个平台根本不支持」与「支持但
这次输入不对」，又不能真的去读日志（journalctl 秒级、log show 更慢、
事件查询也上百毫秒）。
做法：`logs("a\"")` 与 `native(&[])` —— 三平台适配器都把
「特性可用性检查」放在「输入校验」之前（Linux 先 manager 判定、
macOS/Windows 先查命令存在再解析名字），于是：
返回 `Unsupported` = 特性不存在；返回 `InvalidInput` = 特性存在、
只是探针名字故意非法。x-cli 报告把后者归 supported，零副作用。

## 探测只跑安全操作：ping/trace 打环回，DNS 刷新真的刷

环回 ping/trace 不依赖外网、毫秒级返回，能真正区分「ICMP API 被
权限/防火墙挡住」与「实现存在」；`flush_dns_cache` 本机实测返回
PermissionDenied（非管理员），报告如实降级成
「需要管理员权限」——这正是 capability 表最该告诉用户的事。
kill / 服务动作绝不实测：静态报 supported + note
「not exercised, privileges apply」。能力探测把自己跑挂或杀掉服务
是不可接受的失败模式。

## 数据形状探测：字段有没有值，比代码注释更诚实

温度/governor/P-E 核/dhcp 标记/收发计数/卷 UUID/介质类型这些「实现
了但平台未必暴露」的特性，直接对现有数据探测：
`cpu.temperature_celsius` 是 Some 就 supported，None 就 degraded 并
带上原因（本机 Windows：temperature degraded、governor degraded，
与实现知识一致但不靠手抄表格）。风险：Linux 无传感器时也会报
degraded——这是诚实的，报告说的是「这台机器」而不是「这个平台」。

## 遗留与边界

- trace/ping 的探测结果是「环回可达」，不代表外网 ICMP 出站可用。
- macOS `log show` 不在探测路径里（太慢），logs 行的 supported 依据
  是 `log` 存在性检查，实际首查耗时留给用户命令本身。
- 表格状态词（supported/degraded/unsupported）与 serde 拼写一致，
  `CapabilityStatus::name()` 保证脚本解析稳定。
