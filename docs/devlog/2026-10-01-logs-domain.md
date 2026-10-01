# 2026-10-01 系统日志域（x logs）：三套日志体系的接线坑

## PowerShell `Get-WinEvent`：空结果、单结果、坏提供方

- **现象**：`Get-WinEvent -FilterHashtable @{ProviderName='不存在'} -ErrorAction
  SilentlyContinue` 在 zh-CN 主机上依然把错误记录打到 stderr，且
  `powershell.exe -Command` 退出码非 0；错误正文是本地化的（“参数错误。”）。
- **根因**：`-EA SilentlyContinue` 只压住控制台的错误渲染，EventLogException
  仍以 error record 经 CLIXML 到 stderr；原生命令宿主把 exit code 透传出来。
- **处理**：不猜“提供方不存在 → NotFound(3)”——本地化文本没法可靠判别，猜错
  就是编造。适配器把非零退出原样映射为 `Error::system`（exit 1），stderr
  全文进消息，用户看到的正是 PowerShell 自己说的话。空结果集与单结果集是
  另一类坑：`ConvertTo-Json` 对 0 条输出**空串**、1 条输出**对象**、≥2 条
  输出数组，解析端（`parse_winevent_json`）三种都要吃，见
  `crates/x-platform/src/common/logs_os.rs` 的
  `winevent_json_accepts_object_array_and_empties`。
- **证据**：活体三条——`x logs service "Service Control Manager" --limit 2`
  出真事件；`x logs service Nonexistent.Provider.Xyz` stderr 带
  “参数错误”+ exit 1；`x logs process 1234` exit 7。

## `TimeCreated` 直接进 JSON 会变成对象

PS5 把 `DateTime` 序列化成 `{"DateTime":"...","DisplayHint":...}`。脚本里用
计算属性 `@{N='Time';E={$_.TimeCreated.ToString('o')}}` 先把时间钉成
round-trip ISO 串，解析端拿到的一直是字符串。同理级别取
`LevelDisplayName`（中文主机上就是「信息」「错误」）——按既定诚实原则，
平台原文透传，不翻译。

## journald：JSON 逐行里混噪音

`journalctl -o json` 在空 journal 时 stdout 里可能混入
“No journal files were found.” 这类非 JSON 行；解析按行取
`starts_with('{')` 再喂 `serde_json`，噪音行直接跳过。二进制
`MESSAGE` 不是字符串而是一个 JSON 对象（`{"op":"hex",...}`），取
`Value::String` 失败时把整个值 `to_string()` 透传，别丢内容。
`__REALTIME_TIMESTAMP` 是微秒，除以 1e6 后复用 x-core audit 的
`rfc3339_utc`（Hinnant civil-date，早为审计写的 UTC 格式化），不引入
chrono。字段选取顺序 `_SYSTEMD_UNIT → SYSLOG_IDENTIFIER → COMM`，与
journal 自身的展示优先级一致；`_COMM` 受 Linux comm 16 字节截断影响，按名
查进程日志查不到时先想这一点。

## macOS `log show --style json`：键名跨版本漂移

Apple 没给这个 JSON 的兼容承诺，实测各版本见过 `Time`/`Timestamp`、
`Type`/`Level`、`process`/`Process` 混用。解析用
`pick_str(record, &["Timestamp","Time","timestamp"])` 的多键回退，取不到
就 `None`，不编。`log show` 输出**旧→新**，与 `LogPage` 的新→旧契约相反，
解析完 `reverse()` 再 `truncate(limit)`。此路径本机无法活体验证，逻辑靠
单测 `unified_log_records_survive_key_drift` 钉住，真机验证留给 CI runner。

## 分层：serde_json 从 dev-dep 转正式依赖

`x-platform` 原先只在测试里用 serde_json。日志适配器要真解析工具的
JSON 输出，就把它提为 `[dependencies]`（workspace 统一版本），与其同时
保留了 dev-dependencies 里的条目——重复无害，删除会让测试先炸。
Windows 域读日志不逐进程索引：`LogScope::Process` 在 Windows 如实
Unsupported(7)，错误消息里给出替代路径（`x logs service <provider>` /
`x service logs <name>`），这是三平台能力差异第一次被建模成 scope 枚举
而非三条命令三套旗标。
