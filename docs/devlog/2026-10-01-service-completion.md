# 2026-10-01 P0 数据补全：服务 logs 与平台原生逃生舱（三平台）

本批完成路线图 P0 的「服务」项：`ServiceManager` 增加带诚实默认
（Unsupported→退出码 7）的 `logs(name, limit)` 与 `native(args)`；
Linux 用 journalctl、macOS 用 `log show`、Windows 用
`wevtutil qe System /f:xml` 取最近日志（统一最新在前），逃生舱把
参数原样交给 systemctl / launchctl / sc。CLI 新增
`x service logs <name> [--lines N]` 与 `x service native <args>…`。
本机（Windows，zh-CN）实测通过；Linux/macOS 走
`cargo check --all-targets --target`。

## SCM 事件的 EventData 里是本地化显示名，不是服务名

现象：按服务名 `Spooler` 过滤 SCM 事件会漏掉最典型的 7036
（服务启动/停止）——本机事件文本是「Windows 打印后台处理程序」；
7040（启动类型变更）的 param4 反而是短名 BITS。两种事件形态不统一。
处理：XPath 用 `Provider[@Name='<name>' or @Name='Service Control
Manager']` 拉宽，回到代码里再按「任意 Data 值包含服务名 **或**
SCM 枚举到的本地化 display_name（小写包含）」过滤；display_name 由
`status()` 现取，取不到就只按服务名。
证据：`x service logs BITS` 命中 7040 五条；单元测试同时覆盖
「display 命中」与「他人服务被丢弃」。

## 本机 System 日志最近 512 条 SCM 事件没有任何 7036

现象：写「Spooler 必有 7036」的活体测试直接挂 0 条。事件 ID 直方图
显示最近记录被 7040×370、7045×48、7026×40 占满——BITS/TrustedInstaller
启动类型抖动把安静的服务挤出了窗口。
根因：System 日志是全系统共享的，窗口大小本质上是赌命中率。
处理：fetch 上调为 `lines*8` 夹在 [100, 1024]；活体测试改成先原样
查 200 条 SCM 事件、从 Data 里反推一个本机真实存在且有名事件的服务，
再断言 `logs()` 能找到它——不假设任何特定服务。
教训：Windows 事件类活体测试不要写死服务名。

## wevtutil / sc 的输出是 OEM 代码页，不是 UTF-8

现象：zh-CN 机器上 `sc query` 报错、wevtutil 的中文 Data 直接
from_utf8_lossy 会得到乱码。
处理：`decode_console()` 先试 UTF-8（纯 ASCII 与 UTF-8 工具兼容），
失败再走 `MultiByteToWideChar(CP_OEMCP)`。顺序不能反：GBK 字节里
「自动启动」几乎不可能连成合法 UTF-8，反过来 UTF-8 中文在 CP936
下解出来才是真乱码。
证据：`x service logs BITS` 表格正常显示「自动启动 | 按需启动」；
`x service native query NoSuchSvc42` 显示「指定的服务未安装」。

## sc.exe 把 Win32 错误码直接当进程退出码

现象：`sc query NoSuchSvc42` 退出码是 1060（ERROR_SERVICE_DOES_NOT_EXIST），
不是惯常的 1。
处理：逃生舱不翻译退出码——`NativeOutput.exit_code` 原样回传，stdout/
stderr 照打；非零时 x 报 `Error::system("sc exited with code 1060")`
并以自己的稳定退出码 1 结束。JSON 模式先输出完整 NativeOutput 再报错，
脚本能同时拿到文本和码。

## git-bash 会把 wevtutil 的 /q: 参数改成路径

现象：手工验证 XPath 时 `wevtutil qe System "/q:..." /rd:true` 回
「参数太多」或 0 条，看起来像查询写错。
根因：MSYS2 把以 `/` 开头的参数当 Unix 路径转换（改成 Q:/... 之类）。
Rust 里 `Command::output()` 走 CreateProcess 不受影响，纯排查坑。
处理：shell 侧验证一律 `export MSYS2_ARG_CONV_EXCL='*'` 再跑。

## journalctl 短格式的时间戳是 3 个 token，续行没有

现象：多行 journal 消息的后续行原样打印，不带时间戳；若盲目把前 3 个
token 当时间戳，续行会被伪造出一个假 timestamp。
处理：`journal_line()` 取前 3 个 whitespace token，但只有首 token
形如 3 字母月份（"Oct"）才认领时间戳，否则整行进 message。level 诚实
留 None——短格式没有级别字段。
证据：单测分别断言正常行拆出「Oct 05 10:23:41」+ 正文、续行
timestamp=None。

## macOS 统一日志按可执行文件名索引，不是 launchd 标签

背景：`log show --predicate 'process == "NAME"'` 的 process 是
exec 名（如 sshd），`com.openssh.sshd` 这类标签匹配不上；也没有
per-job 日志窗口，本批以「最近 1h + 尾部 N 行」为文档化边界。
注入防护：NAME 进谓词字符串字面量前拒绝引号/反斜杠，Command 不走
shell，参数不二次解析。本机无法实测，靠 `cargo check --all-targets
--target aarch64-apple-darwin` + 纯函数单测（compact 行拆时间戳）。

## 遗留与边界

- OpenRC/SysV 的 logs 明确 Unsupported（退出码 7）：它们的日志是
  自由格式文件，没有可依赖的 schema。
- Windows logs 只查 System 通道：服务自定义 ETW provider 的通道
  千差万别，先不做无根据的猜测；`x service native` 可以补
  `wevtutil qe <channel>`。
- 逃生舱不弹确认框：它是文档化的直通口，权限语义交给管理器本身
  （systemctl/launchctl/sc 自己会拒）。
- `log show` 在 macOS 上秒级慢，TUI 未接入 logs 视图（P0 只要求 CLI）。
