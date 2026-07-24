# CSA v0.2.0 GitHub 正式发布计划

日期：2026-07-24
状态：`APPROVED FOR v0.2.0 STABLE RELEASE`

## 1. 当前判定

本地功能、真实运行时、release 构建、ZIP 哈希、包内验收和只读升级预检均已通过。用户于 2026-07-24 确认 Telegram 文本与图片链路并授权直接合并 `main`、发布正式 `v0.2.0`。正式资产必须满足：

1. 从最终 `main` 干净提交构建，`manifest.json.sourceTreeDirty=false`。
2. ZIP 与 `.sha256` 同时上传，并从 GitHub 回下载复核。
3. Release 资产不得复用本地脏工作树生成的 RC 包。

## 2. 历史候选资产

```text
dist/candidate-v0.2.0-rc1-20260724/
  claude-science-assistant-v0.2.0-rc.1-release-portable.zip
  claude-science-assistant-v0.2.0-rc.1-release-portable.zip.sha256
```

该资产仅用于本地候选验证，不能改名后上传。正式 Release 必须从最终 `main` 重新构建无 qualifier 的资产。

## 3. 提交范围审查

禁止执行 `git add -A`。按功能边界逐组检查并暂存：

### Commit 1: Connect

```text
connect-gateway/
extensions/csa-claude-science-connector/
skills/csa-connect/
vendor/csa-connect/
launcher/src-tauri/src/connect.rs
proxy.py
tests/test_translation.py
```

同时纳入 `launcher/src-tauri/src/lib.rs`、`launcher/src/App.tsx`、`launcher/src/App.css` 中属于 Connect 的部分；这些文件也包含其他功能，必须逐段审查。

建议提交信息：

```text
feat: add local Connect gateway and Claude Science return path
```

### Commit 2: Subagent

```text
skills/csa-external-agent/
scripts/create-subagent-request.ps1
scripts/create-subagent-request.sh
docs/csa-ui-subagent-acceptance-tests.zh-CN.md
docs/subagent-hub-usable-product-plan.zh-CN.md
```

再逐段纳入 `launcher/src-tauri/src/lib.rs`、`launcher/src/App.tsx`、`launcher/src/App.css` 中 Subagent Hub、session 和 outbox 的代码。

建议提交信息：

```text
feat: complete manual-approval Subagent task loop
```

### Commit 3: Release engineering

```text
launcher/package.json
launcher/src-tauri/Cargo.toml
launcher/src-tauri/Cargo.lock
launcher/src-tauri/tauri.conf.json
scripts/verify-new-features.ps1
scripts/verify-v0.2-package.ps1
scripts/package-launcher-portable.ps1
scripts/self-test.ps1
skills/bootstrap-claude-science-wsl/scripts/bootstrap-wsl-runtime.sh
docs/v0.2-*.md
docs/github-release-v0.2.0.md
docs/plans/github-v0.2.0-push-plan.zh-CN.md
README.md
.gitignore
```

建议提交信息：

```text
release: prepare CSA v0.2.0 rc1 package and acceptance gates
```

任何无法确认来源或与 v0.2 无关的脏文件都留在工作区，不为了“干净”而回退用户改动。

## 4. 分支与 PR

从当前工作分支整理提交后推送同一 `codex/` 分支，或在确认所有改动归属后创建：

```powershell
git switch -c codex/csa-v0.2.0-connect-subagent
git push -u origin codex/csa-v0.2.0-connect-subagent
```

PR 必须附：

- Connect 与 Subagent 架构摘要。
- `reports/release-readiness/` 证据结论，不上传含本机路径的原始报告。
- 自动测试结果与 Telegram 验收结论。
- 安全边界和回退步骤。

## 5. 干净构建门

PR 合并后，在最终 `main` 提交执行：

```powershell
git status --short
$env:CARGO_BUILD_JOBS = "1"
.\scripts\verify-new-features.ps1 -LiveConnectRuntime -VerifyProxy
.\scripts\package-launcher-portable.ps1 -Profile release
```

如果上游 Claude Code 可用，再加 `-LiveExternalAgent`。它有 120 秒单轮超时，不会无限等待。

检查：

```text
manifest.version = 0.2.0
manifest.packageQualifier = 空
manifest.sourceCommit = 最终 main commit
manifest.sourceTreeDirty = false
```

任一项不满足，停止发布。

## 6. 发布后烟雾测试

使用最终干净 ZIP 解压到新目录，按以下顺序：

1. 运行 `scripts/verify-v0.2-package.ps1 -LiveConnectRuntime -VerifyProxy`。
2. Telegram 发送唯一文本，确认当前 Claude Science 只收到一次。
3. 确认 Telegram 只收到一份最终回复。
4. Claude Science 生成 PNG，确认 Telegram 收到一张可打开的图片且不重复。
5. 提交 Subagent Demo，批准、完成、读取 outbox，再恢复 session 追问一次。
6. 退出新包并打开旧包，确认回退路径可用；随后恢复新包。

## 7. 正式 Release

1. 创建注释标签 `v0.2.0`。
2. 创建 GitHub Draft，不勾选 Pre-release。
3. 使用 `docs/github-release-v0.2.0.md` 作为正文。
4. 上传 ZIP 与 `.sha256`，不上传解压目录、`.venv`、本机 evidence 或配置文件。
5. 从 GitHub 草稿回下载两个资产并复核 SHA-256。
6. 回下载验收通过后公开正式 Release，并设置为 Latest。

## 8. 发布完成标准

正式包必须由干净 `main` 在不带 qualifier 的情况下构建：

```powershell
.\scripts\package-launcher-portable.ps1 -Profile release
```

`main`、`v0.2.0` 标签、Release 源码提交和 `manifest.sourceCommit` 必须一致；ZIP 与 `.sha256` 回下载复核后才算发布完成。历史 RC 资产保留用于追溯，不静默覆盖。
