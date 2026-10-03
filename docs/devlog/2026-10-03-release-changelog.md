# 2026-10-03 · 发布流程与 CHANGELOG

对应路线图 C 组「发布流程与 CHANGELOG」。`scripts/release-tag.sh` 早就存在了，
缺的是两样东西：仓库里那份 CHANGELOG，和 GitHub Release 页面上那段正文。

## 决策：一份生成器服务两个产物

CHANGELOG 与 Release 正文如果各写一套逻辑，第一件事就是漂移：同一个 tag，
仓库里说这五条改动，发布页上说那六条。所以 `scripts/changelog.sh` 只有一个
`commits_for` + 一个 `emit_groups`，`--release <tag>` 只是换了个外壳（不写
`## 标题`，改写「本区间从哪个 tag 之后算起」，末尾挂资产清单与契约指针）。
CI 的 publish 作业跑的就是 `scripts/changelog.sh --release "$GITHUB_REF_NAME" -o release-notes.md`。

正文不再重复 tag 名：GitHub 已经把它作为 Release 标题显示，正文里再写一遍
`# v0.2.0` 只是两行同义文字。留下来的那句是「Changes since `v0.1.0`」——这是
读者从 tag 名本身**推不出来**的那件事实。

## 坑：生成的文件会把自己列进去

流程本该是「刷新 CHANGELOG → 提交 → 打 tag」。但 release-tag.sh 刷新完、
用户提交这条 CHANGELOG 之后，HEAD 就多了一个提交；再跑一次，待发布一节里
出现 `docs: regenerate CHANGELOG for v0.2.0`，文件又变了，脚本又停下——
永远收不住。

处理：`commits_for` 用 `git log --invert-grep --grep='^docs: regenerate CHANGELOG'`
把生成器自己的产物排除掉（`scripts/changelog.sh:114`、过滤条件在 `:116`）。这条排除
同时作用于 `--release`：一条「更新 changelog」的提交本来也不该出现在发布说明里。
release-tag.sh 停下来时把该执行的命令原样打出来（`scripts/release-tag.sh:207`），
正是为了让用户写出**能被这条过滤规则认出的**信息，而不是各写各的。

另一件相关的事：脚本不替用户提交。CHANGELOG 变了就 exit 1，让用户看过、提交、
再跑一遍。代价是发布多一次往返，换来的是 tag 只会指向被确认过的那棵树——
`git tag` + `git push` 是这条链路上唯一收不回来的动作。

## 坑：CHANGELOG 里没有的提交才是真风险

分组规则只认 `type:` 前缀。而这个仓库的历史里有大量 `C4 …`、`P4 打包分发：…`
这类无前缀（或前缀不在白名单）的信息。若按「不认识就丢掉」写，发布说明会
静默漏掉一批已交付的改动，而漏掉这件事在产物里看不见。所以它们**原样**进
「Other changes」一节，并带一句说明。同理 `!` 只用来判断 breaking，不做别的推断。

## 坑：CI 的检出深度与安装包文件名

publish 作业原来只做 `download-artifact`，没有仓库可读。加了 checkout 之后
默认是浅检出（tag push 时只有那一个提交、且不带 tags），`--release` 会既找不到
前一个 tag 也读不到历史，于是必须 `fetch-depth: 0` + `fetch-tags: true`
（`.github/workflows/release.yml:97`）。

安装包这边有个旧的不一致：`x.iss` 里 `MyAppVersion` 是硬编码的 `0.1.0`，
`OutputBaseFilename` 由它拼出来，所以打 v0.2.0 的 tag 上传的是
`x-0.1.0-windows-x86_64-setup.exe`——正文里承诺的资产名在文件系统上并不存在。
改成工作流把 tag 传给 `ReleaseTag`（文件名用它，`Packaging/Windows/x.iss:38`），
数字部分传给 `AppVersion`
（不该带 `v`），本地编译仍走文件里的 `#ifndef` 兜底值。两个 define 分开是因为
tag 带 `v` 而 AppVersion 不该带。传参走 `env:` 而不是插值进命令行：值来自 tag 名，
不能让一个 tag 给编译器塞参数。

## 验证

- 真实仓库：`scripts/changelog.sh` 对 66 条提交的历史出稿，两次运行 `diff -q`
  字节一致（幂等）；`--heading v0.1.0` 把待发布节改名并带上 HEAD 日期。
- 合成仓库（`/tmp`，三个 tag + 一条 merge + 各种形状的 subject）：
  `feat:` / `fix(core):`（渲染成 `**core:**`）/ `perf!:`（`**breaking**`）/
  `docs:` / `C9 无` / `P4 中文标题：带冒号` / merge 提交。逐 tag 的区间边界正确
  （v0.3.0 只含 v0.2.0 之后的、merge 的另一侧分支提交被列出而 merge 本身不列），
  首个 tag 回退成「整条历史」；`--release` 不存在的 tag → exit 1；`-o` 走
  `tmp.$$` + `mv`，失败留旧文件（`trap` 清临时文件）。
- 收敛性：合成仓库里跑 `--heading v0.5.0 -o CHANGELOG.md` → 文件变脏；按提示
  提交 `docs: regenerate CHANGELOG for v0.5.0` → 再跑一次，`git status --porcelain`
  对该文件为空。这一条是整个流程能自动化的前提。
- **未验证**：本机没有 Inno Setup（`iscc` 不在 PATH，Program Files 下也没有），
  `#ifndef` 与 `OutputBaseFilename` 的改动是纸面审查；CI 的 publish 作业需要一次
  真实 tag 推送才能确认检出深度够用。这两处都属于「第一次发版时看日志」的部分。
- `scripts/check.sh` 全绿（Format / Clippy / Test）。
