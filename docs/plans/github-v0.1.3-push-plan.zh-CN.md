# CSA v0.1.3 GitHub 全量推送计划

计划日期：2026-07-11

目标仓库：`Dalaoyuan2020/claude-science-assistant`

目标版本：v0.1.3

状态：`CONDITIONAL GO / WAITING FOR USER APPROVAL`。本计划获批前，不暂存、不提交、不推送、不打标签、不创建 Release。执行时以 `github-v0.1.3-push-plan-review.zh-CN.md` 和 `github-v0.1.3-push-codex-prompt.zh-CN.md` 的补强护栏为准。

## 1. 当前远端基线

| 项目 | 当前状态 |
| --- | --- |
| 默认分支 | `main` |
| 远端 main | `199b2c74ec58577096634e236da5f0c72e802931` |
| 最新公开 Release | v0.1.2 |
| v0.1.2 Release 资产 | ZIP 与 `.sha256` 均存在 |
| v0.1.3 Release | 不存在 |
| GitHub CLI | 当前未登录，推送前需要重新授权 |

## 2. 本地完整性结论

| 检查 | 结果 |
| --- | --- |
| Bridge translation/security | 51/51 通过 |
| Rust launcher | 37/37 通过 |
| Bridge detection regression | 9/9 通过 |
| `cargo fmt --check` | 通过 |
| TypeScript/Vite production build | 通过 |
| 当前 WSL 运行状态 | `overall=ready` |
| Bridge 来源 | 当前 v0.1.3 DeepSeek thinking 候选包，`source_path` 匹配 |
| 工作区变更 | 37 个已跟踪文件修改，13 个未跟踪公开候选文件 |
| Markdown 相对链接 | 26 份文档，未发现断链 |
| 可发布文件秘密扫描 | 130 个文件，未发现疑似 `sk-...` Key |
| debug 包布局 | EXE、Bridge、Skill、Prompt、更新记录、Claude Science 二进制均存在 |
| Rust 临时目录 | `target-validation/` 已加入 `.gitignore` |

当前 release 候选包仍记录 `sourceTreeDirty=true`，只能作为测试证据，不能直接上传。正式包必须在提交后重新构建。

## 3. GitHub 首页增量更新范围

保留原 README 主体结构，只做以下增量：

1. 版本徽章改为读取 GitHub Latest Release。
2. 增加下载、首次安装、升级、实现原理和绿皮书顶部导航。
3. 增加首次安装 / 从旧版升级双入口。
4. 增加 v0.1.3 的六状态栏、Bridge 来源校验和 DeepSeek thinking 修复说明。
5. 把旧的“只负责首次安装”Prompt 改为读取统一的首次安装 / 升级 Prompt。
6. 文档导航加入 v0.1.3 更新记录与统一 Prompt。
7. 发布前替换两张旧截图：主界面必须显示六状态栏；Provider 截图不得包含 Key。
8. Release 发布后，把 README 状态说明从“v0.1.3 候选”改为“当前稳定版 v0.1.3”。

## 4. 拟公开推送内容

### 4.1 核心源码

- `proxy.py`、`config.example.json`、`requirements*.txt`
- `launcher/src/`、`launcher/src-tauri/` 中的源码、配置和锁文件
- Provider、Bridge、DPAPI、状态探测、接管和异步任务相关修改

### 4.2 脚本与 Skill

- `scripts/` 下的体检、启动、验收、打包、能力探测和卸载脚本
- `skills/bootstrap-claude-science-wsl/` 下的公开 Skill、检查与回滚脚本

### 4.3 测试

- `tests/test_translation.py`
- `launcher/src-tauri/tests/bridge_detection_regression.rs`
- Rust `src/lib.rs` 内单元测试

### 4.4 公开文档

- README、SECURITY、AGENTS、launcher README
- v0.1.3 Release 文案、Provider 矩阵、排错、验收、更新记录
- 首次安装 / 升级通用 AI Prompt
- MiniMax/DeepSeek 适配计划与构建记录
- WSL 存储迁移设计和独立审查报告；两份文件必须在 H1 后立即醒目标记 `DESIGN ONLY / BUILD NO-GO`，暂存审查时人工核对，不得让用户误以为已实现迁移
- GitHub 首页与发布质量设计、当前推送计划

## 5. 明确排除内容

以下内容不进入 Git 历史：

- `config.json`、`.env*`（保留 `.env.example`）、用户设置和 DPAPI 数据；
- `private/`、付费用户 HTML、内部安装手册和个人服务资料；
- `dist/`、`launcher/dist/`、`target/`、`target-validation/`、`.venv/`、缓存和日志；
- API Key、OAuth token、Cookie、Bridge 控制 token；
- `vendor/claude-science/linux-x64/claude-science` 二进制。

Claude Science Linux 二进制只作为完整便携 ZIP 的组成部分上传到 GitHub Release，不提交到源码 Git 历史。

## 6. 执行顺序

### 阶段 A：审批后准备提交

1. 执行 `git fetch origin`，确认远端没有新提交。
2. 使用 `git checkout -b codex/csa-v0.1.3-release origin/main` 从远端基线创建分支；不得把脏的当前分支 HEAD 当作分支起点。
3. 重新截取两张不含秘密的 v0.1.3 实际界面图。
4. 在暂存前向用户逐项确认是否同意公开：`https://10521052.xyz/v1` 及其第三方信任策略、个人微信二维码、答疑群二维码。没有明确同意则停止。
5. 按明确清单逐项暂存，不使用无差别 `git add .`。
6. 对暂存区再次执行秘密、私有目录、二进制和构建产物审查。

### 阶段 B：本地提交

建议拆成三个可审计提交：

1. `feat: stabilize CSA v0.1.3 runtime and provider routing`
2. `test: expand launcher and bridge regression coverage`
3. `docs: publish v0.1.3 guides, update record, and release notes`

提交后要求 `git status` 干净；任何残留的公开源码修改都必须解释，不能带着遗漏进入构建。

### 阶段 C：干净源码重建

1. 再次运行 Bridge、Rust、回归、格式和前端构建。
2. 使用 `CARGO_BUILD_JOBS=1` 构建 release 便携包。
3. 检查 `manifest.json`：版本为 0.1.3、提交号等于最终提交、`sourceTreeDirty=false`。
4. 检查 ZIP 包含 EXE、Bridge、脚本、Skill、Prompt、更新记录和 Claude Science Linux 二进制。
5. 检查 ZIP 不含 private、用户配置、嵌套压缩包或疑似 Key。
6. 校验内置二进制 manifest、ZIP SHA256 和同名 `.sha256`。
7. 从最终包接管当前 Bridge，完成非流式、流式和 Claude Science 真实对话验收。
8. 优先在 309/差异环境完成 takeover 与 DeepSeek thinking 端到端验收；若无法完成，则在 README 和 Release 显式标注“已验证环境为本机 Windows 11 10.0.22631 + Ubuntu-24.04，309/异机端到端尚未实测”。

### 阶段 D：GitHub 推送

1. 恢复 `Dalaoyuan2020` 的 GitHub 授权。
2. 推送 `codex/csa-v0.1.3-release`。
3. 核对 GitHub 分支文件数、关键文件和 README 渲染。
4. 将已审分支合并或快进到 `main`，不覆盖远端未知提交。
5. 在最终 main 提交创建并推送普通注释标签 `v0.1.3`；只有明确确认本机签名环境后才使用签名标签。

### 阶段 E：GitHub Release

1. 使用 `docs/github-release-v0.1.3.md` 创建草稿、非预发布 Release；校验通过前不得公开。
2. 只上传正式构建的 ZIP 与 `.sha256` 两个资产。
3. 通过 GitHub API 核对资产名称、大小和下载地址。
4. 从 GitHub 草稿资产下载 ZIP 与 `.sha256` 到独立校验目录，重新比较 SHA256。
5. 确认 Release ZIP 内确实包含 Claude Science Linux 二进制。
6. 若哈希不一致，保持草稿，使用带 `-r2` 后缀的新资产名重建并追加勘误；禁止同名静默替换，也不改写 main 历史。
7. 只有资产、哈希和包内容全部通过且用户批准发布后，才公开 Release；随后检查 Latest Release 徽章、下载入口和 README 的版本中性状态说明。

## 7. 停止条件

出现以下任一情况立即停止，不推送或不发布：

- 远端 main 在审批后出现未知新提交；
- 暂存区出现 private、配置、Key、日志、运行时缓存或未说明的二进制；
- 任一自动化测试或真实对话失败；
- 最终 manifest 仍为 `sourceTreeDirty=true`；
- ZIP 不含 Claude Science Linux 二进制、Prompt 或更新记录；
- 本地 SHA256 与 GitHub 下载资产不一致；
- GitHub 授权账户不是 `Dalaoyuan2020`。
- 用户尚未明确同意公开内置中转域名及信任策略、个人微信二维码和答疑群二维码。
- 309/异机验收未完成，且 README/Release 也没有明确写出仅本机验证的兜底声明。

## 8. 回退策略

- Git 推送前：不影响远端，删除或保留本地 release 分支均可。
- main 合并前：只撤销 release 分支，不动 main。
- main 合并后、Release 前：使用新的修复提交回退，不改写公共历史。
- Release 草稿校验失败：保持草稿，删除问题资产后使用 `-r2` 新文件名重新构建、复核并追加勘误；再次取得用户批准前不公开。
- Release 已发布后：发现问题时停止推荐下载、如实追加说明并发布修复版本；不静默替换同名 ZIP，不改写公共 main 历史。
- 本机运行时：最终包验收前保留当前可用候选包和旧解压目录，不迁移或删除 WSL 数据。

## 9. 审批项

用户审批本计划后，授权执行以下动作：

- 创建本地 release 分支和三个提交；
- 重建最终 release ZIP；
- 推送公开源码和文档；
- 将已审内容更新到 main；
- 创建 v0.1.3 标签与 GitHub Release；
- 上传 ZIP 与 `.sha256`；
- 发布后下载复核。

审批不包含上传 private、用户配置、API Key、日志、付费 HTML 或其他未列出的本地文件。
