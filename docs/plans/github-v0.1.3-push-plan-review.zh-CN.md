# CSA v0.1.3 GitHub 推送计划审查报告

审查者：独立只读 Review Agent
审查日期：2026-07-11
被审计划：`docs/plans/github-v0.1.3-push-plan.zh-CN.md`（计划日期 2026-07-11，状态 WAITING FOR USER APPROVAL）
对照依据：`docs/plans/github-v0.1.3-publication-design.zh-CN.md` 的发布门槛、`AGENTS.md`、`SECURITY.md`、当前仓库/Git/构建机器真实状态。

本审查为只读，未提交、未推送、未打标签、未创建 Release、未构建、未运行 WSL 变更命令。

---

## 1. 审查结论

- **Verdict: APPROVE（带 5 项补强项）**
- **Push Gate: CONDITIONAL GO** —— 计划本身可执行且与发布设计严丝合缝，但在用户批复前必须补齐 5 处“计划写了但未落地/未点名路径/未给阈值”的空白；不补齐不应开推。
- 一句话原因：推送计划在停止条件、回退策略、`sourceTreeDirty` 机制、秘密排除与“只下载 EXE 禁止升单”等产品红线上一致且可审计，且关键事实（远端基线、`target-validation/` 已忽略、`config.json`/`.env`/`private/` 已忽略、`vendor` 内置二进制不入 Git、`sourceTreeDirty` 由 `git status --porcelain` 实算）经我现场核实均与仓库真实状态吻合；剩余风险集中在几处“覆盖率与命名不一致、阈值未给出、审批后回退与 Release 后再现(download-verify)链条单点”。

重点说明：你这次让我“详细审查推送计划是否能达到要求”，因此本轮聚焦在**可执行性、停推/回退完备性、秘密与资产完整性、以及与公开设计稿 §10 发布门槛的对齐度**；不再重复 WSL 存储迁移那一轮的安全方法论，仅在涉及该资产的对外口径处做交叉校验。

---

## 2. 现场核实结论（计划事实 vs 仓库真实状态）

| 计划断言 | 现场核实 | 结论 |
| --- | --- | --- |
| 远端 main = `199b2c7…802931`（§1 L16） | `git rev-parse origin/main` = 同值 | ✓ 一致 |
| 最新公开 Release = v0.1.2；v0.1.3 不存在（§1 L17-19） | `git tag` 仅 `v0.1.1`/`v0.1.2` | ✓ 一致 |
| `target-validation/` 已加入 `.gitignore`（§2 L36） | `.gitignore:33` 命中 `launcher/src-tauri/target-validation/` | ✓ 一致 |
| 不提交 `config.json`/`.env*`/`private/`/`dist/`/`target/`/缓存/`vendor/.../claude-science` 二进制（§5） | `git check-ignore`：`config.json`(.gitignore:2)、`.env`(.gitignore:3)、`private/`(.gitignore:43)、`vendor/claude-science/linux-x64/claude-science`(.gitignore:37) 均忽略 | ✓ 一致 |
| `manifest.json sourceTreeDirty` 由 `git status --porcelain` 实算（§6C L117, §7 L147） | `scripts/package-launcher-portable.ps1` L154-157：`$SourceDirty = [bool](git status --porcelain | Select -First 1)` | ✓ 机制与停推条件一致，**这是最强的一点** |
| 自动化测试 51/37/9 通过（§2 L26-28） | 与 `docs/v0.1-current-pc-verification.zh-CN.md` 第 5 节记录一致（Bridge 47→51、Rust 32→37 表明本期又有增量） | ✓ 大致一致；注意 §本机证据表是历史快照，最终值以阶段 C 重跑为准 |
| 资产仅 ZIP + `.sha256` 两个（§5 L91、§6E L134） | 与设计稿 §9 一致 | ✓ 一致 |
| 统一首次安装/升级 Prompt（§3 L48、§4.4 L76） | 文件实存于 `docs/prompts/csa-install-or-upgrade-agent-prompt.zh-CN.md`，内容含“只读体检→场景识别→接管/升级→回退，禁止只换 EXE” | ✓ 一致，但**计划未点名其路径**（见 P2-1） |
| DeepSeek thinking 修复已写入 Release 文案（§2 L32） | `docs/github-release-v0.1.3.md` L23-25 含 `thinking.type=auto→adaptive` 修复 | ✓ 一致 |
| `scripts/probe-provider-capabilities.ps1` 为可计费探测、不日常运行（AGENTS.md L53） | 脚本用 DPAPI `Unprotect-ApiKey` 读取密文 Key，未发现明文 Key 写 stdout/文件 | ✓ 安全策略一致 |
| 截图需替换为六状态栏且不含 Key（§3 L50、§6A L99） | 现存 `docs/assets/screenshots/csa-home.png` 与 `csa-add-api-key.png` 时间为 07-07（早于 2×3 六状态栏 UI）→ 确属旧图，必须重截 | ✓ 计划判断正确 |

核实中发现的**不一致点（非阻断，需在计划里收敛）**：
- 设计稿 §1 L16 写“36 个已跟踪文件有修改”，现场 `git status` 现为 37 个修改 + 5 个未跟踪目录/文件，属于计划写稿与现场漂移（见 P2-3）。
- 当前工作分支为 `codex/bridge-detection-v0.1.2`，而非计划 §6A L98 将创建的 `codex/csa-v0.1.3-release`——这是**符合计划**的（计划打算新建分支），仅提示执行时确认从远端 `origin/main` 而不是当前本地分支拉新分支（见 P1-1）。

---

## 3. 关键缺口（建议补强后再 GO）

### P1-1 阶段 A 第 2 步“从当前远端基线创建分支”未点名起点，存在从“脏分支”而非 `origin/main` 开分支的风险

- 证据：计划 §6A L98“从当前远端基线创建 `codex/csa-v0.1.3-release` 分支”。
- 触发条件：当前本地分支 `codex/bridge-detection-v0.1.2` 含 37 项未提交修改；若执行者从“当前分支”而非 `origin/main` 开新分支并提交，会带入本不该进 v0.1.3 的内容，且与设计稿 §1“整理提交”要求相悖。
- 建议：把该步明确为 `git checkout -b codex/csa-v0.1.3-release origin/main`（或先 `git fetch origin` 再以 `origin/main` 为基底），并要求执行者记录起点 commit。

### P1-2 阶段 E 的“从 GitHub 重新下载并比对 SHA256”缺少操作级阈值与回退动作，停推条件未覆盖“下载哈希不一致但已发布”

- 证据：§6E L136 L49 “核对 SHA256”，§7 停止条件 L149 “本地 SHA256 与 GitHub 下载资产不一致”。但若不一致发生在 **Release 已创建之后**，§7 只说“停止”，未说“是否撤下 Release / 标记 draft / 发布勘误”。
- 触发条件：上传成功但 GitHub 端对象因网络/缓存返回不同字节（少见但存在），下载复核发现差异。
- 建议：§6E 补“若下载复核不一致：立即把 Release 改为 draft 并重试上传同名或带 `-r2` 后缀的资产，发布说明加勘误”，并把它写进 §8 回退策略“Release 发布后”一行（目前 §8 L158 已写“不静默替换同名 ZIP”，方向正确，但需把“draft + 二次复核”显式化）。

### P1-3 阶段 C 的“使用 `CARGO_BUILD_JOBS=1`”缺少为什么与失败处置，易被当可选冗余

- 证据：§6C L118 “使用 `CARGO_BUILD_JOBS=1` 构建 release 便携包”。
- 触发条件：执行者不了解单线程构建是为了规避本机偶发 Tauri/Rust 并行打包抖动，跳过或并行构建导致 manifest/产物不一致。
- 建议：补一句“原因：规避并行构建偶发的资源争用导致打包不一致；若失败重试 1 次后仍失败则停推并记录”。

### P1-4 计划把“WSL 存储迁移设计与独立审查报告”列入公开推送（§4.4 L78），口径仅一句“继续醒目标记 DESIGN ONLY / BUILD NO-GO”——未规定如何保证读者一眼识别

- 证据：§4.4 L78；审查报告（上一轮产出）`wsl-storage-migration-review-result.zh-CN.md` 内含 NO-GO 结论与 P0 清单。
- 触发条件：用户从 Release/仓库读到迁移相关文档，误以为 v0.1.3 已实现“一键迁移整个发行版”，叠加 README“WSL 存储”状态项，产生与 v0.1.0“换机不可用”类似的过度预期。
- 建议：在 §4.4 补一条“这两份文档的 H1 下首行与 README 文档导航行必须出现 `DESIGN ONLY / BUILD NO-GO` 字样”，并要求阶段 A 的暂存审查包含此项人工核对。

### P1-5 §7 停止条件未覆盖“LICENSE 密钥或第三方中转 Base URL 被写进历史/Release 说明”

- 证据：§7 停推条件只列了 private/配置/Key/二进制/`sourceTreeDirty`/二进制缺失/哈希不一致/账户错；未列“内置中转 `https://10521052.xyz/v1` 与‘群/个人微信’二维码等 PII/商业元素是否准备好公开”。
- 触发条件：README L114 已含内置中转 Base URL；联系章节含微信二维码。一旦公开，等于把中转域名与个人联系方式永久进 Git 历史。
- 建议：§7 增列“公开前确认内置中转域名、第三方中转信任策略、个人微信与答疑群二维码已被用户明确同意对外公开”；这本身不是阻断，但属“发布前必须显式确认”项，避免事后无法撤回。

---

## 4. 覆盖率与可达性评估

按“设计稿 §10 正式发布门槛”逐条对账推送计划覆盖度：

| 设计稿 §10 门槛 | 推送计划覆盖位置 | 评估 |
| --- | --- | --- |
| 1. 审查并提交源码 | §6A/§6B | ✓ 覆盖；建议补起点（P1-1） |
| 2. `git status` 干净 | §6B L111、§6C L117、§7 L147 | ✓ 三处冗余核对，强 |
| 3. 从目标提交重跑测试+release 构建 | §6C L115-116 | ✓ 覆盖 |
| 4. manifest 正确提交且 `sourceTreeDirty=false` | §6C L117、§7 L147 | ✓ 覆盖，且机制经核实 |
| 5. 对 ZIP 秘密扫描/必需文件/内置二进制哈希 | §6C L119-120、§6A L101 | ✓ 覆盖 |
| 6. 候选目录 Bridge 接管+流式/非流式真实对话 | §6C L121 | ✓ 覆盖；建议补“含一次 Claude Science thinking 真实对话”以对齐本期 DeepSeek 修复卖点 |
| 7. 用户审批 README/Release/资产名/哈希 | §9 审批项 L162 | ✓ 覆盖 |
| 8. 恢复 GitHub 授权后推送源码/标签/Release | §6D、§6E | ✓ 覆盖；账户判据见 §7 L150 |
| 9. 发布后下载复核 | §6E L136-137 | ✓ 覆盖；补阈值（P1-2） |

**结论：设计稿 §10 九项门槛在推送计划中全部有落点，覆盖达标。** 唯一“强度不足”项是门槛 6（真实对话）：本期主打 DeepSeek `thinking=auto→adaptive` 修复，验收里应至少包含一次带 thinking 的真实 Claude Science 对话，否则“DeepSeek thinking 修复”这一 Release 文案卖点未被端到端复现（见 P2-2）。

---

## 5. 安全与资产完整性评估

- **秘密排除**：`config.json`、`.env*`、`private/`、`dist/`、`target/`、`target-validation/`、`vendor/.../claude-science` 二进制经 `.gitignore` 核实均已忽略；`scripts/probe-provider-capabilities.ps1` 不向 stdout/文件写明文 Key。推送计划的排除清单与现场一致。✓
- **资产完整性**：`manifest.json` 由 `package-launcher-portable.ps1` 生成，`sourceCommit`=`git rev-parse HEAD`、`sourceTreeDirty`=`git status --porcelain` 首行存在性——计划“提交后构建 → manifest dirty=false”链路机制真实可信。✓
- **签名/标签**：§6D L129 “签名或普通注释标签”——可接受，但若选签名标签需确认本机有 GPG/keyless signing 且与 GitHub 配置一致；否则建议明确为“普通注释标签”，避免执行时卡在签名环境。见 P2-4。
- **公开边界（需用户显式同意）**：内置中转域名 `https://10521052.xyz/v1`、个人微信二维码、答疑群二维码会随 README 进公开历史——见 P1-5，必须作为审批项明示。

---

## 6. 回退策略评估

§8 回退分了五段（推送前 / main 合并前 / main 合并后 Release 前 / Release 发布后 / 本机运行时），分层合理，且 Release 发布后“不静默替换同名 ZIP”与安全实践一致。两点补强：

1. **“main 合并后、Release 前”使用新的修复提交回退，不改写公共历史**正确，但未说明“若合并已推送但仍可创建补救分支/force-discuss”边界。建议补一句“已推送的 main 不 force-push 改写，只走追加提交或勘误”（进一步固化）。
2. **本机运行时回退**（保留旧解压目录）正确，但与正在并行进行的“WSL 存储迁移”工作流冲突：上一轮审查已指出迁移锁会阻塞升级接管。本推送计划未声明“v0.1.3 发布期间不得在某会话内进行存储迁移测试”，建议在 §8 本机运行时一行补“本次验收与发布期间不触发任何 WSL 存储迁移/接管实验，以免迁移状态机与发布回退互相耦合”。

---

## 7. 可泛化性 / 跨机一致性（呼应用户一贯关注点）

- 推送计划是“发布工程”而非“产品跨机逻辑”，但其 §6C L121 的“从最终包接管当前 Bridge、完成流式非流式真实对话”是**仅在本机验证**的。v0.1.3 是否在 309/其它 Windows 上同样“接管旧 Bridge + DeepSeek thinking”可用，推送计划未要求。
- 这正是 `docs/v0.1-current-pc-verification.zh-CN.md §7`“尚未完成的发布门槛”第 3 条“在 309 或另一台不同环境电脑上执行完整升级/安装验收”的遗留项。**推送计划未把 309 验收列为停止/前置条件**。
- 建议：在 §7 停止条件或 §6C 增一条“若安排了 309/异地验收，需其 `overall=ready` + 接管成功回单后再执行 §6D；若无法安排异地验收，应在 Release 说明或 README 标注‘已验证环境：本机 Windows + Ubuntu-24.04’，避免重演 v0.1.0 换机不可用预期差。”（见 P2-5——这一条最贴近你“软件是通用的、换机能不能用”的关注）

---

## 8. 必答问题逐项（自定义 6 问）

1. **推送计划是否达到设计稿 §10 的发布门槛？** 是，九项门槛均落点且机制经核实，仅门槛 6 建议补“含 thinking 的真实对话”，以及 309 验收前置缺失（P2-5）。
2. **停止条件是否足以在真实风险出现时立即停推？** 基本足够，但缺 Release 后哈希不一致的处置（P1-2）与公开边界确认项（P1-5）。
3. **回退策略是否覆盖所有阶段？** 覆盖五段，需补“已推送 main 不改写”与“发布期禁迁移实验”两句（§6 评估点 1-2）。
4. **秘密与资产完整性是否可信？** 可信，`sourceTreeDirty` 机制与 `.gitignore` 均现场核实一致。
5. **是否会在公开历史里泄漏不应公开内容？** Key/二进制/配置已被忽略排除；但内置中转域名、个人/群二维码会进历史，须作为审批项显式同意（P1-5）。
6. **是否对“换机可用性”有交代？** 计划未把 309/异地验收列为前置，建议补一条“已验证环境”声明，与 v0.1-current-pc-verification §7 第 3 项呼应，防止 v0.1.0 教训在公开后重现（P2-5，关键）。

---

## 9. 最终修改建议（落地清单，按优先级）

1. **P1-1**：§6A 第 2 步写明 `git checkout -b codex/csa-v0.1.3-release origin/main`（先 fetch）。
2. **P1-2**：§6E/§8 补“Release 后哈希不一致 → draft + 二次复核 + 勘误；不改写已推送 main”。
3. **P1-4**：§4.4 补“迁移设计与审查报告须在 H1 首行与 README 导航行维持 `DESIGN ONLY / BUILD NO-GO`，暂存审查人工核对”。
4. **P1-5**：§7 增列“公开前显式确认内置中转域名、第三方中转信任策略、个人微信/答疑群二维码已被用户同意对外公开”。
5. **P2-5（关键）**：§6C 或 §7 增一条“309/异地验收前置，或在不具备时在 README/Release 显式标注‘已验证环境=本机 Windows + Ubuntu-24.04’”。
6. **P2-1**：§4.4 点名统一 Prompt 路径 `docs/prompts/csa-install-or-upgrade-agent-prompt.zh-CN.md`。
7. **P2-2**：§6C 第 7 步补“含一次 DeepSeek thinking 真实 Claude Science 对话”，对齐 Release 卖点。
8. **P2-3**：§2 的真实现场数（37 修改 + 5 未跟踪）替换设计稿 §1 的“36”近似值，以免执行者误判。
9. **P1-3**：§6C 第 2 步补 `CARGO_BUILD_JOBS=1` 的理由与失败重试上限。
10. **P2-4**：§6D 第 5 步明确“普通注释标签”或确认本机签名环境，避免执行时卡签名。

---

## 10. 最终结论

推送计划是一份**可批准、可执行、且与发布设计稿严丝合缝**的工程计划；关键事实（远端基线、`.gitignore` 覆盖、`sourceTreeDirty` 真实机制、统一 Prompt 已就绪、DeepSeek thinking 文案已就绪、旧截图必须重截）经现场核实全部为真，安全性、资产完整性、回退分层均达专业发布水准。

唯一**影响 GO 判定**的硬项是 **P1-1（新建分支起点）、P1-4（迁移文档对外口径）、P1-5（公开边界显式同意）**；以及**与你一贯关注点直接相关的 P2-5（309/异地验收前置或显式环境标注）**——若不补，v0.1.3 在公开后世面遇到“换机不可用/误以为支持迁移”的预期差时，将没有预先声明的缓冲。

因此结论为：**APPROVE / Push Gate = CONDITIONAL GO**：计划可批准执行，但开推前先补齐第 9 节 1–5 项（其中 P2-5 是你“软件通用、换机能不能用”这一关注的最小落地动作），其余 6–10 项作为发布期改进一并落实即可。

残余风险：309/异地真实升级与 DeepSeek thinking 端到端在异机的行为未实测；本机 `dist/` 仅候选包，正式包需按 §6C 在干净提交后重建；本审查为只读 Review，未实际执行推送/构建。