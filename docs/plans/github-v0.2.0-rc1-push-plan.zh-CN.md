# CSA v0.2.0-rc.1 GitHub 推送计划

日期：2026-07-24
状态：`LOCAL RC GO / PUBLIC RELEASE NO-GO`

## 1. 当前判定

本地功能、真实运行时、release 构建、ZIP 哈希、包内验收和只读升级预检均已通过。当前候选包可以在本机测试，但不能直接上传为公开 Release，原因是：

1. 工作树包含大量未提交的 Connect、Subagent、UI、Bridge 和文档改动。
2. 当前候选 `manifest.json.sourceTreeDirty=true`，无法证明资产对应唯一提交。
3. 真人 Telegram 唯一文本与图片闭环需要对最终干净构建再复验一次。

## 2. 候选资产

```text
dist/candidate-v0.2.0-rc1-20260724/
  claude-science-assistant-v0.2.0-rc.1-release-portable.zip
  claude-science-assistant-v0.2.0-rc.1-release-portable.zip.sha256
```

该资产仅用于本地候选验证。提交后必须重新构建，不能把当前 ZIP 改名后上传。

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
docs/plans/github-v0.2.0-rc1-push-plan.zh-CN.md
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
- 自动测试结果与待人工 Telegram 验收清单。
- 安全边界和回退步骤。

## 5. 干净构建门

PR 合并后，在最终 `main` 提交执行：

```powershell
git status --short
$env:CARGO_BUILD_JOBS = "1"
.\scripts\verify-new-features.ps1 -LiveConnectRuntime -VerifyProxy
.\scripts\package-launcher-portable.ps1 -Profile release -PackageQualifier rc.1
```

如果上游 Claude Code 可用，再加 `-LiveExternalAgent`。它有 120 秒单轮超时，不会无限等待。

检查：

```text
manifest.version = 0.2.0
manifest.packageQualifier = rc.1
manifest.sourceCommit = 最终 main commit
manifest.sourceTreeDirty = false
```

任一项不满足，停止发布。

## 6. 人工验收门

使用最终干净 ZIP 解压到新目录，按以下顺序：

1. 运行 `scripts/verify-v0.2-package.ps1 -LiveConnectRuntime -VerifyProxy`。
2. Telegram 发送唯一文本，确认当前 Claude Science 只收到一次。
3. 确认 Telegram 只收到一份最终回复。
4. Claude Science 生成 PNG，确认 Telegram 收到一张可打开的图片且不重复。
5. 提交 Subagent Demo，批准、完成、读取 outbox，再恢复 session 追问一次。
6. 退出新包并打开旧包，确认回退路径可用；随后恢复新包。

## 7. 草稿 Release

1. 创建注释标签 `v0.2.0-rc.1`。
2. 创建 GitHub Draft、勾选 Pre-release。
3. 使用 `docs/github-release-v0.2.0.md` 作为正文。
4. 上传 ZIP 与 `.sha256`，不上传解压目录、`.venv`、本机 evidence 或配置文件。
5. 从 GitHub 草稿回下载两个资产并复核 SHA-256。
6. 回下载验收通过后公开 RC。

## 8. 稳定版

RC 验收完成且没有阻断性问题后，在干净 `main` 上不带 qualifier 重建：

```powershell
.\scripts\package-launcher-portable.ps1 -Profile release
```

创建 `v0.2.0` 正式标签与非预发布 Release。RC 资产保留用于追溯，不静默覆盖。
