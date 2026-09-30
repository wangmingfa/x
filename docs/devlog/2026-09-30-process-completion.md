# 2026-09-30 P0 数据补全：进程详情与 ps tree 折叠（三平台）

本批完成路线图 P0 的「进程」项：`ProcessInfo` 增加 `cwd` /
`open_files` / `connections` / `environment`，`ps show` 输出详情，
`ps tree --collapsed` 折叠子树。macOS 实机验证，Linux/Windows 走
`cargo check --target` + CI。

## sysinfo 0.39 能给什么、不能给什么

- `Process::cwd()` 和 `environ()` 可用；`environ()` 是 `KEY=VALUE`
  平铺的 `&[OsString]`，要自己 `split_once('=')`（值里也可能有 `=`，
  所以只切第一个）。
- **`open_files()` 只返回数量不是列表**——名字容易让人以为是 fd 列表。
- **`Process` 上没有 `connections()`**，逐进程 socket 表不存在。
  所以打开文件列表和连接必须平台原生实现：
  - macOS：`proc_pidfdinfo(PROC_PIDFDVNODEPATHINFO)` 拿 fd 路径，
    `PROC_PIDFDSOCKETINFO` 拿每 fd 的 socket 详情（port.rs 已有）。
  - Linux：`/proc/<pid>/fd` 的 symlink 过滤掉 `socket:`/`pipe:`，
    连接复用 port.rs 的 `/proc/net` join。
  - Windows：两者都需要驱动级/提升 API（NtQuerySystemInformation /
    Restart Manager），本期置空集合并注明，不假装进程没有打开文件。
- 深度字段只在 `get(pid)`（单进程详情）采集，list 路径保持轻量——
  逐进程读 fd 表会把 `x ps` 变成 `lsof` 的成本。

## macOS fd 路径的结构体偏移

带路径的 flavor 是 `PROC_PIDFDVNODEPATHINFO`（=2），返回
`struct vnode_fdinfowithpath`（1200 字节）：`pfi` 24 字节 +
`vnode_info_path`，路径在整体偏移 `24 + 152 = 176`，长 `PATH_MAX`。
成员链是 `pvip.vip_path`（不是 `vi`/`vip`），用 clang `offsetof`
探针核实后才敢写常量——沿用 CPU 批次的教训：**先探 SDK 再写偏移**。

## CLI 细节

- `ps tree` 的 `Tree(PsListArgs)` 改成 struct 变体带 `--collapsed/-c`；
  折叠行显示 `name (+N hidden)`，N 用现成的 `depth_total()`。
- `ps show` 的连接表/打开文件/环境变量放在主表之后，空集合不输出，
  但字段存在时即使为空也认为「平台有能力、进程恰好没有」。
