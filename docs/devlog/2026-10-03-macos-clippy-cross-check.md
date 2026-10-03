# 2026-10-03 · macOS 专属代码：本地闸门看不见，直到我用 aarch64-apple-darwin 跑了一遍

CI 的 `macos-latest | Clippy` 从 run 33 起连红 7 次（run 29–32 红在 `Test`，那是
`5cdd791` 已经修掉的 SIGBUS）。日志要鉴权才拿得到，用户贴出原文后是同一个 lint 五处：

```
error: field assignment outside of initializer for an instance created with Default::default()
   --> crates/x-platform/src/macos/smc.rs:130:9
```

`clippy::field_reassign_with_default`，全在 `crates/x-platform/src/macos/smc.rs` 的
`key_count` / `key_name_at` / `read_key` 里。

## 为什么本地一直是绿的

三层遮蔽叠在一起：

1. `macos/smc.rs` 在 `#[cfg(target_os = "macos")] pub mod macos;` 底下
   （`crates/x-platform/src/lib.rs:33`），Windows 上的 `cargo clippy` **根本不编译它**，
   连语法都不检查。
2. CI 的两个交叉检查步骤（`.github/workflows/ci.yml:35`、`:41`）用的是
   `cargo check --target ... -p x-platform`，不是 clippy——它们只保证「别的平台的
   代码能编过」，不带 lint。
3. 于是 macOS 专属的那份代码，全套 lint 只在那台 macOS runner 上跑过一次。
   本地闸门绿不代表闸门覆盖到了它。

`common/smc.rs` 是跨平台的（协议、结构体布局、`sp78` 解码都在那儿，各平台都 lint），
坏的恰好是只剩 Mach 管道的那一半。

## 修法

按 lint 自己的建议改成结构体字面量 + `..Default::default()`，嵌套的
`key_info.data_size` 一并写进字面量里，不再「先 default 再逐字段补」。字节结果不变：
`Default` 把 80 字节全清零，字面量只是覆盖其中几个字段。

## 这台 Windows 上怎么验的（关键发现）

`rustup target list --installed` 里已经有 `aarch64-apple-darwin`，而
**clippy 只做 check、不做链接**，所以 `#[link(name = "IOKit", kind = "framework")]`
那一串外部声明在 Windows 上照样能过类型检查：

```
cargo clippy --target aarch64-apple-darwin --all-targets --all-features -- -D warnings
```

反向验证走了一遍，两次都真的重编了 `x-platform`（输出里有 `Checking x-platform`）：

| 文件版本 | 退出码 | 结果 |
| --- | --- | --- |
| `git checkout HEAD --` 回到改前 | 101 | 五处 `field_reassign_with_default`，行号 130/138/148/157/165，与 CI 报的完全一致 |
| 修好后（`cp` 回来 + `touch`） | 0 | 干净 |

本机 clippy 是 1.98.0、CI 是 1.99.0，但行号与 lint 名逐条对得上，说明这条红就是这五处、
没有别的。`bash scripts/check.sh` 也 EXIT=0。

## 还没做

把这条交叉检查接进 `scripts/check.sh|ps1`（比如 cargo 里已经有 apple target 才跑，
跑不了就打印「跳过：本机没有 aarch64-apple-darwin」）。那需要改闸门的语义，先只记在这儿。
