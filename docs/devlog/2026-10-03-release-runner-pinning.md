# 2026-10-03 · 发布工作流：把 `-latest` 钉死，把「常用系统」讲成可测的事实

需求两条：不要用 latest 版本的操作系统；常用操作系统要兼容进去。

## 先查现在到底有哪些镜像（2026-10-03 抓 `actions/runner-images`）

不能凭记忆钉版本号——钉到一个即将下线的镜像，等于给发布流程埋一颗哑雷。抓到的现状：

- Ubuntu：`ubuntu-26.04` / `ubuntu-24.04`（= `ubuntu-latest`）/ `ubuntu-22.04` 都在；
  **ARM64 只有 `ubuntu-24.04-arm` 与 `ubuntu-26.04-arm`**，没有 22.04-arm。
- macOS：只剩 arm64 一档（`macos-26` = `macos-latest`，`macos-15` 可用，
  `macos-14` 已打 deprecated 标记）。**GitHub 不再提供 Intel macOS runner**。
- Windows：`windows-2025`（= latest）/ `windows-2022`，另有 `windows-11-arm`。

## 矩阵

| 作业 | 镜像 | 目标 | 产物 |
| --- | --- | --- | --- |
| `linux-x86_64` | `ubuntu-22.04` | host | `x-<tag>-Linux-x86_64.tar.gz` |
| `linux-aarch64` | `ubuntu-24.04-arm` | host | `x-<tag>-Linux-aarch64.tar.gz` |
| `macos-universal` | `macos-15` | `aarch64-apple-darwin` + `x86_64-apple-darwin` | `x-<tag>-macOS-universal.tar.gz` |
| `windows-installer` | `windows-2022` | host MSVC x64 | `x-<tag>-windows-x86_64-setup.exe` |

macOS 那一格是关键：**没有 Intel runner 不等于没有 Intel 产物**。同一个 SDK 交叉编
`x86_64-apple-darwin`，再 `lipo -create` 合成一个 fat binary，一台 arm64 机器就覆盖
Apple Silicon + Intel，`MACOSX_DEPLOYMENT_TARGET=11.0` 是 Apple Silicon 自身的下限。
本项目没有需要 C 编译的依赖（`core-foundation-sys` / `libc` / `windows-sys` 都是纯
Rust 绑定），同 OS 跨 arch 因此不需要额外工具链。

## 我第一版写错、随后收回的一句话

初稿把镜像直接写成兼容性结论：「`ubuntu-24.04-arm` → 需要 glibc ≥ 2.39，Debian 12
arm64 因此落在外面」。**构建机的 glibc 版本不是产物要求的版本**——纯 Rust 二进制引用到的
`GLIBC_*` 符号常常比构建机自带的那版老好几轮，那条结论是我从镜像名推出来的，没有测过。
改成让产物自证：Linux 那一步 `objdump -T` 打印引用到的最高三个 `GLIBC_*`，macOS 那一步
打印 `lipo -archs` 和 `otool -l` 里的 `minos`。README 与 Release 正文也只说「built on
Ubuntu 22.04」这种构建机事实，不再替用户下「你需要 glibc 2.35」的判语。

## 另外三处顺手修的坑

1. **publish 作业原来没有 `permissions: contents: write`。** 仓库把默认 token 设成只读时，
   `softprops/action-gh-release` 会在这里失败——而这个作业从来没跑过（仓库还没有 tag），
   所以这是颗还没被踩到的雷。
2. **产物名原来用 `github.ref_name`**，`workflow_dispatch` 起来时会把分支名当版本号
   （`x-main-Linux.tar.gz`）。installer 作业专门处理了这一点，两边现在同一规则：无 tag 就是
   `v0.0.0-dev`。
3. **publish 增加「正文必须逐字点名实际产物」的自检**：从 `release-notes.md` 里 grep 出
   `` `x-<tag>-…` `` 与 `artifacts/` 下的文件排序对比，不一致退出 1。生成的说明承诺一个
   下载页上没有的文件，比构建失败更糟。模式里带上 tag，避免把某条 commit 标题里反引号包住的
   `x-core` 之类的东西误当成产物。

Windows 留在自己的作业里没并进矩阵，是因为 `Packaging/Windows/x.iss` 的 `SourcePath` 写死
`..\..\target\release\x.exe`，而矩阵步骤一律带 `--target`，产物会落在
`target/<triple>/release/`——两边路径必须有一个让步，让给 Inno 更省事。既然它自己重编，
原来那条 `needs: build` 就只是排队：任何一条 Linux 失败都会把 Windows 产物一起拖没，
而两者之间没有数据依赖。这一条去掉了，publish 仍然 `needs: [build, windows-installer]`，
缺任何一边都不发布。

## 验证

- `npx --yes js-yaml` 解析通过；再用 dump 脚本核对 jobs / `runs-on` / `needs` /
  `permissions` / 三个 matrix include 的每个键。
- 正文↔产物自检跑了四种情形（合成 repo，tag `v9.9.9`）：名字对得上 exit 0；改名 exit 1；
  多一个文件 exit 1；正文里根本没有产物名（把 `GITHUB_REF_NAME` 设成无关 tag）时仍然打出
  「promise: (空) / produced: …」的差异然后退出 1——这一条是给 `set -eo pipefail` 下
  `grep` 无匹配就中止补的 `|| true`，不然步骤会在报出有用信息之前就死掉。
- `cargo build --release --locked -p x-app --target x86_64-pc-windows-msvc` 在 Windows 上
  产出 `target/x86_64-pc-windows-msvc/release/x.exe`，矩阵里 `bindir` 的布局据此写的。
- `bash scripts/check.sh` EXIT=0。

## 未验证

整个工作流仍然一次没跑过（仓库没有 tag）。以下都要等第一次 `workflow_dispatch`：
`ubuntu-24.04-arm` 这个 label 的拼写与可用性、`objdump -T` / `otool -l` 的输出形状、
universal 产物在老 macOS 上的实际表现。

## 没做，留给你选

- Linux 下限真要压低就得上 musl（`*-unknown-linux-musl` 静态）或 `cargo-zigbuild`；
  钉镜像只是让基线可复现，不会让产物更老。
- Windows ARM64 原生产物（`aarch64-pc-windows-msvc` 交叉 + Inno 6.3 的 `arm64`）。
  现在 x64 包在 ARM Windows 上靠模拟运行。
- `x.iss` 没写 `MinVersion`，Windows 7/8.1 用户也装得上去，跑不跑得起来是另一回事。
- `ci.yml` 仍在用 `-latest`。测试作业跟着镜像漂不至于坏，但 macOS clippy 的行为会随
  runner 的 Xcode/clippy 版本变（这次五条 lint 就是 1.99 报的）。
