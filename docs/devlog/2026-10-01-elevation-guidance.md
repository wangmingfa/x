# 2026-10-01 权限提升细化：hint 按真实主机措辞

## 需求与现实的错位谁来兜底

各适配器上报的是**它自己语法里的权限要求**：Unix 路径说 `Root`，
Windows 路径说 `Administrator`，低端口占用这类计划说 `Elevated`。但
x-core 的 `From<io::Error>` 不分平台地把 EACCES 标成 `Root`——在
Windows 主机上跑出来的“Try running with sudo.” 就是把 Linux 的话术
硬套给用户。修法不是在 x-core 里加 cfg（架构守卫禁止 x-core 有任何
OS 知识），而是新增 `PermissionRequirement::platform_guidance(os)`：
前端拿着错误里上报的 requirement，再对照 context 报告的**真实
OsFamily**，把错配当场纠正——`Root` + Windows 渲染成
“没有 root 账户，请用提升到管理员的终端（Win+X → Terminal (Admin)）”，
而不是假装 sudo 有用。

## 文案要求“可执行”，不是同义反复

- Linux/Root：sudo 之外给出被拒时的排障路径（sudoers /
  `usermod -aG sudo $USER`）——这是“not in the sudoers file”用户
  真正的卡点。
- macOS/Root：sudo 成功仍被拒时提示 系统设置 → 隐私与安全
  （Full Disk Access），对应 sudo 兜不住的那类拒绝。
- Windows：给 Win+X 的入口名与 `net localgroup Administrators`
  自检命令；Elevated 单列“令牌缺特权”的说法，与“需要管理员”区分。
- `platform_guidance` 是 `const fn`，(requirement, os) 全矩阵穷举，
  单测遍历 5×4 保证没有空格子；同时钉一条“Windows 上绝不出现
  sudo”的断言，防止将来有人图省事回退成统一文案。

## 落点

`x-cli::report()`（错误人话路径）改为带 `Option<OsFamily>`：`run_with`
有 context 就传 Some，`create_context` 本身失败时退回通用
`guidance()`——那时连“真实主机”都还没拿到，不能装作知道。CLI 测试
harness（tests/cli.rs 自己复刻 report）同步换成
`platform_guidance(context.os())`，并加 `privilege_hints_are_worded_for_the_reported_os`
用 `with_system` 伪造 Linux/macOS/Windows 三种 SystemInfo，验证同一条
Root 拒绝在三种主机上各说各话。TUI 无提权错误呈现路径，不动。
`x capability` 的 degraded 注记保留通用 `guidance()`：能力报告描述
“本机特性状态”，不指挥用户下一步动作，通用措辞反而准确。
