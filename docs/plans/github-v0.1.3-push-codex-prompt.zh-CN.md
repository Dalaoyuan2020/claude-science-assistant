# CSA v0.1.3 GitHub 推送：交给 Codex 的执行 Prompt

> 这是给 Codex / Claude Code / 其他具备本机命令能力的 AI 助手的执行 Prompt。
> 角色边界：Codex 是本计划的**执行者**，不是审批者。`CONDITIONAL GO` 是审查结论，不等于用户已经授权执行。只有用户明确批准本 Prompt，并完成 §2 的公开边界确认后，才允许进入会改变 Git/GitHub 状态的步骤；授权之外的任何发布、撤销或对外动作都必须停下并征求用户确认。
> 工作区根：当前仓库根目录。先用 `git rev-parse --show-toplevel` 获取并核对，不把本文档作者的本机路径当成可移植事实。

---

## 0. 必读文件（先全读再动手）

请按顺序完整阅读以下文件，不要跳读：

1. `docs/plans/github-v0.1.3-push-plan-review.zh-CN.md` ← **本轮外部审查报告，最高优先级**。其中第 9 节"最终修改建议（落地清单）"是你必须在执行中落实的补强项。
2. `docs/plans/github-v0.1.3-push-plan.zh-CN.md` ← 推送计划本体，你按它的章节结构执行，但被审查报告修改/约束的地方以审查报告为准。
3. `docs/plans/github-v0.1.3-publication-design.zh-CN.md` ← 发布设计稿，尤其 §10 正式发布门槛是验收底线。
4. `docs/v0.1-current-pc-verification.zh-CN.md` ← 本机已验证证据与尚存发布门槛（309/异地验收是未决项）。
5. `AGENTS.md`、`SECURITY.md`、README 的"从旧版升级""安全与隐私边界"两节。
6. `docs/plans/wsl-storage-migration-review-result.zh-CN.md` 的审查结论（不执行，只了解必须维持 `DESIGN ONLY / BUILD NO-GO` 的对外口径与本发布的关系）。

确认以上文件真实存在后再继续。不要凭记忆推断文件状态。

---

## 1. 红线（违反任一立即停止并向用户报告）

- **不执行任何 WSL 存储迁移、`--move`、`--export`、`--unregister`、`--import`、卸载或重建发行版动作**。v0.1.3 发布期与存储迁移工作流互斥（见审查报告 §6 回退评估第 2 点）。
- **不输出、不写入、不截图、不上传、不提交** API Key、OAuth token、Cookie、Bridge 控制 token、`config.json`、`.env*`、`private/` 内容、付费用户 HTML。
- **不修改 Clash、VPN、DNS、hosts、Windows 系统代理、根证书或 443 端口。**
- **不使用 `git add .` / `git add -A`** 这样的无差别暂存；只能按明确清单逐项 `git add <path>`。
- **不 force-push 改写已推送的 `main` 公共历史**。已推送内容只走追加提交或勘误。
- **GitHub 账户必须是 `Dalaoyuan2020`**；恢复授权后先用 `gh api user --jq .login` 核对，不是则立即停止。
- **不静默替换已发布的同名 ZIP**；任何资产更正都走 `draft`+二次复核+勘误。
- 任一阶段出现审查报告 §7 停止条件中任一项，**立即停下并汇报**，不要尝试"绕一下"。

---

## 2. GO 前必须先补的 5 项前置补强（审查报告第 9 节 1–5 项）

在进入 §6A 阶段 A 之前，先在本工作区完成下面这 5 项前置补强（这些是对计划本体的修订/落地，不是可选改进）：

1. **P1-1 新建分支起点**：把推送计划 §6A 第 2 步落地为
   ```bash
   git fetch origin
   git checkout -b codex/csa-v0.1.3-release origin/main
   ```
不要把当前本地分支 `codex/bridge-detection-v0.1.2` 的 HEAD 当作新分支基线；新分支必须指向当时重新获取的 `origin/main`。未提交文件数量必须现场读取，不使用本文中的历史数字。记录起点 commit 并在汇报里附上。

2. **P1-4 迁移文档对外口径**：在 `docs/plans/wsl-storage-migration-plan.zh-CN.md` 与 `docs/plans/wsl-storage-migration-review-result.zh-CN.md` 的 H1 下一行确认/补上 `状态：DESIGN ONLY / BUILD NO-GO` 字样；并在 README 文档导航涉及这两份的条目里保留"设计稿/未实现"措辞。阶段 A 暂存审查要把"这两份文档首行可一眼识别为未实现"作为人工核对项之一。

3. **P1-5 公开边界显式同意**：在真正暂存、`git commit` 或推送前，停一次向用户**显式征求同意**——README 公开后会把内置中转域名 `https://10521052.xyz/v1` 及其“第三方中转、需确认域名、非官方服务”的信任策略、个人微信二维码、答疑群二维码永久写入公开 Git 历史。请逐项列出并请用户确认“同意对外公开”，得到明确回复后再继续。**若用户未明确同意，停在阶段 A 暂存前，不提交、不推送。**

4. **P1-2 Release 哈希不一致的处置**：Release 必须先以 `draft` 创建并上传资产，下载复核通过后才能公开。若阶段 E 发现本地 SHA256 与 GitHub 资产不一致，保持 `draft`，移除有问题的资产并重新构建；新资产必须使用带 `-r2` 后缀的新文件名，在发布说明中追加勘误并再次取得用户批准。禁止用同名文件静默替换，已推送的 `main` 绝不 force-push 改写。

5. **P2-5（关键）换机可用性兜底声明**：按审查报告第 9 节第 5 项二选一执行：
   - **优先**：若时间允许，在 309 或另一台差异环境实机跑一次" takeover + DeepSeek thinking 真实对话"，回单 `overall=ready` 后再推 §6D。或
   - **兜底**：在最不济的情况下，在 README 顶部透明状态说明与 Release 文案显式标注"已验证环境 = 本机 Windows 11 + Ubuntu-24.04；309/异机升级与 DeepSeek thinking 端到端尚未实测"，并由此避免重演 v0.1.0 换机不可用的预期差。

完成可在本地安全落实的补强后，向用户汇报五项状态，并逐项展示 §2 第 3 项的公开内容。只有用户明确回复“同意对外公开并执行”或同等清晰授权，才进入阶段 A；仅回复“计划看过”不视为执行授权。

---

## 3. 执行流程（按推送计划 §6 的 A→E，逐阶段汇报）

### 阶段 A：审批后准备提交
- A1 重新获取 `origin/main`，确认远端无新提交（`git fetch origin` 后比对 `origin/main` 是否仍为 `199b2c74ec58577096634e236da5f0c72e802931`；若变则停——见审查报告 P1-1 未远端漂移点）。
- A2 按 §2 第 1 项从 `origin/main` 建 `codex/csa-v0.1.3-release` 分支。
- A3 重新截两张不含秘密的 v0.1.3 实际界面图：`docs/assets/screenshots/csa-home.png`（六状态栏 2×3）、`csa-add-api-key.png`（用 DeepSeek/MiniMax 已适配界面，**严禁含明文 Key**）。现存截图为 07-07 旧版，必须替换。
- A4 按清单逐项暂存（不用 `git add .`）。务必包括此前未跟踪的：
   - `docs/github-release-v0.1.3.md`
   - `docs/v0.1.3-update-record.zh-CN.md`
   - `docs/prompts/csa-install-or-upgrade-agent-prompt.zh-CN.md`（统一 Prompt，审查报告 P2-1 建议点名纳入）
   - `scripts/probe-provider-capabilities.ps1`
   - `docs/plans/` 下的 v0.1.3 发布相关与迁移设计/审查报告（迁移相关保持 `DESIGN ONLY / BUILD NO-GO` 标记）
- A5 对暂存区再跑一次秘密/私有目录/二进制/构建产物审查（`git ls-files` 不应含 `private/`、`config.json`、`.env`、`dist/`、`target/`、`target-validation/`、`*.exe`、`vendor/.../claude-science` 二进制）。

### 阶段 B：本地提交
- B1 拆成三个可审计提交（文案见计划 §6B）：
   1. `feat: stabilize CSA v0.1.3 runtime and provider routing`
   2. `test: expand launcher and bridge regression coverage`
   3. `docs: publish v0.1.3 guides, update record, and release notes`
- B2 提交后 `git status` 必须干净；任何残留公开源码修改都要在汇报里解释，不能带遗漏进入构建。

### 阶段 C：干净源码重建
- C1 重跑 Bridge 51 项 / Rust 37 项 / 桥接回归 9 项 / `cargo fmt --check` / Vite production build，逐项把实测数字贴进汇报。
- C2 用 `CARGO_BUILD_JOBS=1` 构建 release 便携包（审查报告 P1-3：理由是规避并行打包偶发不一致；失败重试 1 次仍失败则停推并记录）。
- C3 检查 `manifest.json`：`version=0.1.3`、`sourceCommit`=最终提交号、`sourceTreeDirty=false`。**这是 `sourceTreeDirty` 机制经我现场核实的最硬门——不达标就停（计划 §7 L147）。**
- C4 ZIP 须含 EXE、Bridge、scripts、Skill、Prompt、更新记录、Claude Science Linux 二进制；不得含 `private/`、用户配置、嵌套压缩包、疑似 `sk-...` Key。
- C5 校验内置二进制 manifest、ZIP SHA256、同名 `.sha256` 一致。
- C6 从最终包接管当前 Bridge，完成**非流式 + 流式 + 一次 Claude Science 带 DeepSeek thinking 的真实对话**（审查报告 P2-2：thinking 真实对话对齐本期 Release 卖点，缺一不可）。

### 阶段 D：GitHub 推送
- D1 恢复 `Dalaoyuan2020` 授权；`gh api user --jq .login` 必须回 `Dalaoyuan2020`，否则停（计划 §7 L150）。
- D2 推 `codex/csa-v0.1.3-release`。
- D3 核对 GitHub 分支文件数、关键文件、README 渲染（含六状态栏截图、首次安装/升级双入口）。
- D4 合并或快进到 `main`，**不覆盖远端未知提交**；若远端 main 已漂移则停。
- D5 在最终 main 提交创建并推送标签 `v0.1.3`（审查报告 P2-4：默认用普通注释标签，若要用签名标签须先确认本机签名环境）。

### 阶段 E：GitHub Release
- E1 用 `docs/github-release-v0.1.3.md` 创建**草稿、非预发布**的 Release；草稿验证完成前不得公开。
- E2 只上传正式构建的 ZIP 与 `.sha256` 两个资产。
- E3 用 GitHub API 核对资产名称、大小、下载地址。
- E4 使用已核验的 `Dalaoyuan2020` 账户从 GitHub 草稿资产下载 ZIP 与 `.sha256` 到**独立校验目录**，重新比对 SHA256；不一致则按 §2 第 4 项处置（保持 draft + `-r2` 新文件名 + 二次复核 + 勘误，不改写已推送 main）。
- E5 确认 Release ZIP 内确实含 Claude Science Linux 二进制。
- E6 只有 E3–E5 全部通过且用户批准发布后，才把草稿发布为正式 Release；随后检查 Latest Release 徽章、下载入口与 README 的版本中性状态说明。

---

## 4. 汇报格式（每个阶段结束都按此回给用户）

每完成一个阶段，回给用户一段，必须含：

1. 当前阶段（A/B/C/D/E）与已完成步骤编号。
2. 关键实测数字（测试通过数、`sourceCommit`、`sourceTreeDirty`、ZIP SHA256、`gh api user` 账户）。
3. 是否触发任一停止条件；若触发是哪一条、做了什么处置。
4. 下一阶段将做什么，是否需要用户在此节点再次确认。
5. 若 §2 第 3 项（公开边界）仍在等待用户确认，**显式再次询问**，不要自行假定同意。

---

## 5. 完成后的最终汇报

全部阶段结束后，最终汇报必须含：

- 推送的最终 `main` commit、`v0.1.3` tag、Release URL、两个资产名与 SHA256。
- 同时分别列出本地最终 ZIP SHA256 与从 GitHub 草稿/正式资产重新下载后的 ZIP SHA256；两者必须完全相同。
- `manifest.json` 的 `sourceCommit` 与 `sourceTreeDirty=false` 证据。
- 阶段 C 测试全通过、阶段 E 下载复核 SHA256 一致的证据。
- §2 第 5 项（换机兜底）的落实方式：是做了 309 异机验收，还是落实了"已验证环境"公开声明。
- 仍存在的残余风险（309/异机 DeepSeek thinking 端到端是否实测、内置中转域名与二维码是否被公开等），请如实列出，不要粉饰。

---

## 6. 一句话授权边界

**你被授权执行：补齐前置 5 项 → 阶段 A–E 的提交、重建、推送、标签、Release、资产上传与下载复核；授权不含：动 WSL 迁移、公开任何 secret、force-push main、静默替换同名 ZIP、或任何本 Prompt 未列出的对外动作。任一超出清单的动作都先停下问用户。**
