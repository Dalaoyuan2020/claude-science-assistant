# CSA WSL 存储迁移方案独立审查报告

状态：`DESIGN ONLY / BUILD NO-GO`，仅供方案审查；v0.1.3 不实现、不执行 WSL 存储迁移。

审查者：外部独立审查 Agent（只读 Review）
审查日期：2026-07-11
被审方案：`docs/plans/wsl-storage-migration-plan.zh-CN.md`（Draft 1，状态 DESIGN ONLY）
本报告未经任何迁移、停止 WSL、导出、导入、unregister、注册表修改或构建操作。

---

## 1. 审查结论

- **Verdict: CONDITIONAL APPROVE**
- **Build Gate: NO-GO**
- 一句话原因：方案在“不做一键无备份迁移、官方 Move 优先、不自动 unregister、计划指纹 + Codex 逐阶段确认”的方向是安全的，但原方案对 v0.1.0 类教训（只对开发机版本高度兼容、换机即不可用）没有显式吸收，存在多项会导致“换一台 Windows / 老版本 WSL 上桥接打不开或误判成功”的 P0/P1 缺口，且升级路径与存储迁移互相纠缠未被厘清，Build 前必须补齐兼容性矩阵、官方语义引用与升级语义。

关键审查重点说明：本轮在 Prompt 既有维度之外，按用户明确要求加重两个维度——
- **泛化性/可复用性**：方案是否对任意一台 Windows 电脑（Win10/Win11、Store WSL 与 inbox 旧 WSL、Ubuntu-22.04/24.04、systemd 开/关、C/D/E/H、中文空格长路径、BitLocker）都能站得住，而不是只对当前开发机（E 盘、Ubuntu-24.04、Store WSL、WSL 2.x 支持 `--manage`）高度兼容——这正是 v0.1.0“换机桥接打不开”教训的重演风险点。
- **升级路径操作性**：全新安装本就简单，真正的风险是“已有 CSA + 已有发行版 + 大量用户数据”的用户如何升级/替代；这部分在存储迁移方案里与 Bridge 所有权接管、配置 revision、`source_path` 校验深度耦合，但原方案没有把它当成一个一等公民来处理。

---

## 2. 审查范围与证据

### 2.1 已读文件（确认真实存在）

1. `docs/plans/wsl-storage-migration-plan.zh-CN.md`（被审方案，Draft 1）
2. `AGENTS.md`
3. `SECURITY.md`
4. `README.md`（存储、升级“从旧版升级”、安全与隐私边界章节）
5. `launcher/src-tauri/src/lib.rs`（重点读 420–960 行：`windows_storage_snapshot`、`discover_distros`、`preferred_distro`、`current_status`、Bridge `source_path`/`config_revision` 校验 2095–2139 行）
6. `launcher/src/App.tsx`（`storageDetail`、首页“WSL 存储”状态项 649–705 行）
7. `launcher/src/App.css`
8. `scripts/status-probe.ps1`
9. `scripts/start-claude-science-wsl.ps1`
10. `scripts/start-claude-science-wsl.sh`
11. `skills/bootstrap-claude-science-wsl/SKILL.md`
12. `skills/bootstrap-claude-science-wsl/scripts/inspect-windows.ps1`
13. `skills/bootstrap-claude-science-wsl/scripts/inspect-wsl.sh`
14. `skills/bootstrap-claude-science-wsl/references/rollback.md`（注意：Prompt 要求读 `references/rollback.md`，实际文件名一致；另存在 `support-matrix.md` 与 `result-schema.md`，一并用于核实兼容边界）
15. `docs/v0.1-current-pc-verification.zh-CN.md`

补充阅读（用于泛化性/升级路径判断）：
- `docs/v0.1-clean-pc-acceptance.zh-CN.md`（跨机器验收流程，含 309/干净机检查项）
- `skills/bootstrap-claude-science-wsl/references/support-matrix.md`（官方支持矩阵）
- `dashboard_url_from_config` 等端口/token 处理（lib.rs 2142+）

### 2.2 使用的官方资料（附 URL 与访问日期）

wsL 命令语义只采用微软官方来源：

- WSL 基本命令（含 `--export`/`--import`/`--import-in-place`/`--unregister`/`--shutdown`/`--terminate`）：
  https://learn.microsoft.com/en-us/windows/wsl/basic-commands （访问 2026-07-11）
  关键事实摘录：
  - `wsl --export <Distro> <FileName>`：“Exports a snapshot of the specified distribution as a new distribution file. Defaults to tar format.” 可用 `--vhd` 导出为 .vhdx（仅 WSL2）。
  - `wsl --import <Distro> <InstallLocation> <FileName>`：把 tar 导入为新发行版；`--vhd`/`--version` 可选。**导入产生的是“新发行版”，身份须重建。**
  - `wsl --import-in-place <Distro> <FileName>`：“Imports the specified .vhdx file as a new distribution. The virtual hard disk must be formatted in the ext4 filesystem type.” —— **明确要求目标是 ext4 文件系统的 .vhdx，且以“new distribution”身份导入。**
  - `wsl --unregister <Distro>`：“Once unregistered, all data, settings, and software associated with that distribution will be permanently lost. Reinstalling from the store will install a clean copy.” —— 官方明确定性为**不可逆、全部丢失**。
  - `wsl --shutdown`：终止所有运行中的发行版及 WSL2 轻量 VM（影响范围是“全部”）。
  - `wsl --terminate <Distro>`：终止指定发行版（精确作用域）。
  - 更改默认用户：`<Distro> config --default-user <User>` 仅对带启动器的发行版有效；**导入发行版无启动器，必须改 `/etc/wsl.conf`**。

- WSL 磁盘空间与 `--manage` / VHD 管理：
  https://learn.microsoft.com/en-us/windows/wsl/disk-space （访问 2026-07-11）
  关键事实摘录：
  - “The `wsl --manage` command is only available to WSL releases 2.5 and higher.” —— 官方对 `--manage` 的可用性给出明确版本门槛 **WSL 2.5+**。
  - 现有公开文档中 `wsl --manage` 的明文用例是 `--resize`；`--move` 在 basic-commands 与 disk-space 两份官方文档中**均未出现**。
  - WSL2 VHD 默认上限 1TB，文件系统为 ext4，文件名 `ext4.vhdx`。
  - 重要警告：“We recommend that you do not modify, move, or access the WSL related files located inside of your `AppData` folder using Windows tools or editors. Doing so could cause your Linux distribution to become corrupted.” —— 官方对“用 Windows 工具搬/碰 vhdx”给出腐蚀风险警告。
  - 只读回退：VHD 遭遇 mounting error 会以 read-only 回退，需 `wsl --mount --vhd --bare` + `e2fsck` 修复。
  - 定位 vhdx 的官方脚本与 CSA 现有 `windows_storage_snapshot` 一致：读 `HKCU:\...\Lxss` 的 `BasePath` + `\ext4.vhdx`。

- WSL 高级配置（`wsl.conf` / `.wslconfig`，含 systemd、默认用户、sparseVhd）：
  https://learn.microsoft.com/en-us/windows/wsl/wsl-config （访问 2026-07-11）
  关键事实摘录：
  - “The 8 second rule”：配置变更后必须等子系统完全停止再重启，典型约 8 秒，`--shutdown` 是全量快路径但会影响全部发行版，`--terminate <Distro>` 可精确停单个。
  - `sparseVhd`（实验项）：“When set to `true`, any **newly created** VHD will be set to sparse automatically.” —— 即稀疏属性只对新建 VHD 生效，对已存在并 move 过来的 vhdx **不自动转为稀疏**。
  - `[wsl2]` 等键路径须用转义反斜杠 Windows 路径。
  - 升级/重启需 `wsl --shutdown` 才让 `.wslconfig` 生效。

- WSL GitHub Releases（用于核实 `--move` 历史）：
  https://github.com/microsoft/WSL/releases （访问 2026-07-11）
  关键事实摘录：在 2.6.3–2.9.3 的条目中只见 `MoveDistribution` 内部 bug 修复（2.7.3 “Fix VHD ownership after cross-volume move to prevent `E_ACCESSDENIED`”；2.9.3 “Fixed `MoveDistribution` failing with `E_ACCESSDENIED` when setting the VHD owner under impersonation”），**未在发布说明里看到面向用户的 `--move` flag 推广文案**。

### 2.3 未能验证的事实（必须标“未知/不可依赖”）

1. **`wsl --manage <Distro> --move <Location>` 的官方语义**：在 basic-commands 与 disk-space 两份官方文档中均无 `--move` 条目。仅能间接推断 `MoveDistribution` 能力在 2.7.3 以前跨卷移动会触发 `E_ACCESSDENIED`，2.9.3 才修复 impersonation 下的同类错误。因此下列语义**官方文档未明确保证，方案不得当作已验证事实使用**：
   - Move 是否保留发行版名/GUID/默认用户/默认发行版状态/WSL 版本。
   - Move 失败时原 VHD/原注册是否一定保留、是否可能留下半移动状态。
   - Move 是否要求先 `--shutdown`/`--terminate`（文档对 `--resize` 明确要求 shutdown，合理推断 Move 也需停机，但官方对 Move 未给出明文停机要求）。
   - 目标路径是否必须是空目录、是否接受中文/空格/长路径、是否拒绝网络盘。
   - 跨卷（C→D/E/H）Move 在 Win10 19045 旧 Store/inbox WSL 上是否相同行为。
2. **`--import` / `--import-in-place` 之后默认用户、默认发行版、systemd 是否被恢复**：官方只保证“导入为新发行版”，不保证默认用户保留；导入发行版无 launcher，`config --default-user` 不可用，须靠 `/etc/wsl.conf` `[user] default=` 恢复——方案所言“记录默认用户/`/etc/wsl.conf`”方向正确，但官方未保证 import 会自动套用。
3. **Store 安装发行版 vs 手动 import 发行版在 Win10/Win11 的注册表差异**：官方对 `BasePath`/GUID 位置只在 disk-space 给了一个查询脚本，未穷举 Win10 19045 inbox WSL 与 Win11 Store WSL 在 Lxss 注册表布局上的差异。
4. **是否需要管理员权限执行 `--manage --move`**：官方未在标准文档中明确，需在目标机实测量化。

因此，凡依赖上述未验证语义的设计，必须按 Prompt 要求标为“未知/不可依赖”，不能以开发机一次性成功作为证据。

---

## 3. P0 发现

### P0-1 官方 `--manage --move` 语义未被方案引用，且其保留/失败行为官方未保证，方案据此作“官方 Move 是安全主路径”存在根本地基缺失

- 严重级别：P0
- 证据：方案 §1 目标版本建议 v0.1.4（辅助迁移）；§6.1“官方Move路径（优先）”以 `wsl --manage <Distro> --move <Location>` 可用为适用条件；§6.1 只写“Move失败时不得自动切换到包含 unregister 的兼容路径”，但未引用 `--move` 的官方版本门槛、保留内容、停机要求与失败语义。审查方在微软官方 basic-commands 与 disk-space 两份文档中**未检索到 `--move` 条目**；`--manage` 整体官方明文为“WSL releases 2.5 and higher”（disk-space 页）。
- 触发条件：任意非当前开发机的目标机器上 `--move` 因 WSL 版本低于 2.5、或因 Store/inbox 差异而不可用/语义不同；或 Move 失败后留下半移动状态（参考 WSL 2.7.3 “Fix VHD ownership after cross-volume move to prevent `E_ACCESSDENIED`”——说明跨卷移动在历史版本会出错且需专门修复，并非天然稳妥）。
- 最坏后果：把“未知语义的官方命令”当作“可靠主路径”写入 plan.json 与 Codex Prompt，在真实用户机器上 Move 失败/半失败后，因为方案没有官方级别的“原发行版与原注册必然保留”保证，后续基于“原发行版仍注册”的恢复分支可能在半移动状态下执行，导致整个发行版不可用或数据不可达。
- 为什么现有保护不足：方案只做了“失败不自动 unregister”这一层；但这是针对兼容路径的，对官方 Move 自身的失败/半移动状态没有任何官方引用与处理定义。方案 §3 节“只写 Move 优先”等于把地基建立在一份未被官方文档确认的命令语义上。
- 可验证修改建议：
  1. 在方案 §6.1 引用 `--manage` 官方门槛“WSL 2.5+”（disk-space 页 URL）并显式声明：**`--move` 的保留内容（发行版名/GUID/默认用户/默认发行版/WSL 版本/sparse）官方文档未明确，方案以“等价于发行版原地保留，仅改 VHD 物理位置”为假设，并在风险表标注为“假设，非官方保证”**。
  2. 把 Move 成功的判据从“命令退出码 0 + re 读取 BasePath 指向目标”升级为“命令退出码 0 且目标 vhdx 可读、发行版可启动、默认用户未变、原 BasePath 下已无活动 vhdx”多证据闭环（见 P0-5）。
  3. 把 309/VM 实验项写入 §16：在临时测试发行版上实测 `--move` 的保留内容与跨卷语义，否则 `--move` 不应作为首版默认主路径，应降级为“在确认支持的机器上提供，否则回退到纯只读诊断 + 建议用户手动迁移”。
- 对应测试/验收：在 309 的 Ubuntu-22.04 + 旧 WSL、以及一台 Win11 Store WSL 上各跑一次临时测试发行版的 `--move`，记录发行版名/默认用户/systemd/默认发行版状态前后是否一致；不在真实用户发行版上跑。

### P0-2 兼容 Export-Import 路径里的 `--unregister` 与方案 §6.2 的“先生成可验证完整备份”顺序，未覆盖“备份 ≠ 可恢复”，存在静默 unregister 导致数据丢失的路径

- 严重级别：P0
- 证据：方案 §6.2 列出 `wsl --export` → `wsl --unregister` → `wsl --import/--import-in-place`；并要求“先验证导出文件存在、大小合理、可读取”。但“可读取/大小合理”不等于“可成功 import 回可运行发行版”。
- 触发条件：导出过程因空间、断电、WSL 写入中途失败，生成一个“大小看起来合理但末尾被截断”的 tar/vhdx；或导入的目标盘文件系统/配额导致 import 阶段才暴露问题；此时已执行 `--unregister`。
- 最坏后果：官方明确 `--unregister` 是“permanently lost”（basic-commands 页）。一旦在“未真正可恢复”的备份上执行 unregister，用户整个发行版、家目录、密钥、其他项目全部永久丢失，且无官方回滚（重装只能得到干净拷贝）。
- 为什么现有保护不足：方案只校验“文件存在 + 大小合理 + 可读取头”，没有要求**在不触碰原发行版的前提下，先用导出文件在一个临时名下 `--import-in-place`/`--import` 起一个验证副本并跑通基本启动**，再执行 `unregister`。“大小合理”是必要非充分条件。
- 可验证修改建议：
  1. §6.2 增加“不可旁路的恢复可行性验证”步骤：在 unregister 之前，用导出文件以**临时发行版名 + 临时目录**导入一份并验证可启动、默认用户正确、VHD 可挂载；只有该验证副本通过，才允许对原发行版执行 unregister。
  2. 明确 v0.1.4“完全可以只生成兼容方案而不内置执行器”，并把“自动 unregister”列为**永不自动执行**，与 §9 Codex 边界第 8 条对齐并升级为方案级硬约束。
  3. 在 §7 兼容迁移里把“完整备份是强制条件”细化为“经过临时导入验证的完整备份”，而非仅“文件存在且大小合理”。
- 对应测试/验收：在一个 1–2GB 临时测试发行版上，构造“导出被截断/末尾字节损坏”场景，证明方案能在 unregister 前拦截即可。

### P0-3 跨机器泛化性缺失：方案用“当前机器 `wsl.exe --help` 是否声明 `--manage`”做特性检测，但对“老 Win10 inbox WSL / Store WSL / Ubuntu-22.04”的无 `--move` 或 `--move` 行为不同的情况没有明确的产品分支，复现了 v0.1.0 “只对开发机版本兼容”的教训

- 严重级别：P0（直接命中用户强调的“软件是通用的、换机能不能用”）
- 证据：方案 §6“不要仅按版本号猜测能力……以 `wsl.exe --help` 是否声明 `--manage <Distro> --move <Location>` 为主要特性检测”。但 §1/§15 测试矩阵虽列出了 Win10 19045 + Ubuntu-22.04，并未规定“当 `--manage` 不可用或 `--move` 子项不存在”时的产品分支语义。现有代码 `discover_distros`（lib.rs 426）与 `inspect-windows.ps1` 也只过滤 docker-desktop，未区分 Win10 inbox WSL 与 Store 版。
- 触发条件：309 场景（Win10 19045 + 老 inbox WSL）上 `wsl --help` 不含 `--manage`，或含 `--manage` 但不含 `--move`；Store WSL 2.5+ 有，inbox 旧 WSL 没有。
- 最坏后果：v0.1.0 教训重演——在开发机（E 盘、Store WSL、有 `--move`）验证“迁移可用”，发布后大量用户机型不满足前提，“辅助迁移”入口在该机器上要么误判可用、要么无明确退路；用户感知“桥接打不开/迁移按钮无响应”，与 v0.1.0“换机桥接打不开”同构。
- 为什么现有保护不足：方案 §4.1 只列了“WSL1 / BasePath 不可解析 / 根只读 / WSL 无响应”等拒绝条件，没有把“本机 WSL 能力不足（无 `--move` 且无误兼容执行器）”列为**显式拒绝 + 明确指引**。
- 可验证修改建议：
  1. 在 §4.1 增加“当本机 `wsl --help` 不声明 `--manage` 或不声明 `--move` 时：入口降级为只读诊断 + 人工迁移指引 + 建议升级 WSL，**不生成可执行迁移 plan**”，并把它写进 `plan.json` 的 `migration_method = unsupported`。
  2. 在 `inspect-windows.ps1`/`lib.rs::discover_distros` 层补一条“WSL 发行通道（Store vs inbox）+ `wsl --version` 是否可用”检测，用于决定 `--manage` 能否信赖。
  3. §15 测试矩阵把“Win10 19045 inbox WSL 无 `--manage`/`--move`”列为必测，且通过标准是“入口明确降级、不误报可用”。
- 对应测试/验收：在 309 实测 inbox WSL 的 `wsl --help`，证明入口在该机不生成“官方 Move”可执行 plan。

### P0-4 升级路径未与存储迁移解耦：方案“辅助迁移”会让一个“只是想升级 CSA 便携包”的用户被推入“迁移整个发行版”的危险流程，且未说明“就地升级路径”是否被迁移状态机阻塞

- 严重级别：P0（直接命中用户强调的“已有旧版本怎么升级/替代”）
- 证据：README“从旧版升级”定义的升级路径是“并排解压新版 + 双击 exe + 新版接管同发行版内旧目录 Bridge + 校验 `source_path`/config revision”，全部发生在**同一发行版内部**，不需要迁移 VHD。而迁移方案 §10 状态机规定“迁移锁存在时，普通启动、Provider 切换、Bridge 重启和卸载必须拒绝运行”。`start-claude-science-wsl.sh` 的 `stop_stale_bridge_listener`/systemd 重注册会在迁移锁下被拒绝。
- 触发条件：用户拉到新版想“升级”并打开；但首页因 `storageWarning`（C 盘 WSL）同时弹出“辅助迁移”入口，用户点进去、生成了 plan/partial 状态、`status.json` 进入 `backup_in_progress`/`move_in_progress` 中途崩溃；之后用户回到升级流程，双击新版 exe 想“接管旧 Bridge”——被迁移锁拒绝运行。
- 最坏后果：用户“升级被一个未完成的迁移状态卡住”，既升不了级也回不去，且因为迁移状态机尚未实现（v0.1.4 才做），现状下更可能演化成“状态文件残留 + 行为不一致”。
- 为什么现有保护不足：方案 §10 只说“迁移锁存在时拒绝普通启动”，但没有定义“升级/接管”是普通启动的子集还是独立类目，也没有定义“用户从未真正确认迁移、只是生成了 plan”时如何安全退出迁移并回到升级路径。§16 待确认第 1 条只问“是否只做生成方案+Codex执行”，没有回答“升级路径如何在迁移状态存在时可用”。
- 可验证修改建议：
  1. 在方案里显式区分两条路径并相互隔离：**(A) 就地升级**（同发行版、同 VHD、只换 CSA 便携包目录与 Bridge 所有权）与 **(B) 存储迁移**（物理搬迁整个发行版 VHD）。明确两者互斥：迁移锁存在时拒绝升级接管，但必须提供“放弃/清理已生成 plan、回到就地升级”的一键出口。
  2. 增加“安全取消迁移”状态：从 `plan_generated`/`preflight_verified` 到 `not_needed`/`eligible` 的可回退路径，并保证取消时**不删除任何备份、不动 VHD**，只清理迁移私有状态。
  3. 把“入口容忍度”写入 §4：`storageWarning` 弹出的“辅助迁移”入口必须可忽略、不阻塞任何升级与启动动作；迁移是 opt-in，不是升级的前置闸。
- 对应测试/验收：构造“用户生成迁移 plan 后中途放弃、双击新版 exe 升级”的剧本，通过标准为升级可正常完成、迁移 plan 被安全留档且不阻塞。

### P0-5 迁移成功判据里“原位置不存在仍被注册使用的活动 VHDX”与 `source_path`/8 秒规则未被官方语义锚定，存在“两副本同时被当成活动实例”的回归

- 严重级别：P0
- 证据：方案 §11 后验检查要求“原位置不存在仍被注册使用的活动VHDX”；但官方 disk-space 警告“do not modify, move, or access the WSL related files … using Windows tools”且 wsl-config 页“8 second rule”声明停机-重启需要等待子系统完全停止。现有 `inspect-wsl.sh` 的 `bridge_source_matches` 只比较 9876 的 `source_path` 与 CSA 项目目录，不校验 VHD 物理位置归属。
- 触发条件：Move 之后旧 VHD 在原 BasePath 仍在（部分移动/稀疏残留），同时新位置也有副本；8 秒未到用户/启动器重连，`source_path` 仍指向旧 CSA 目录，但 VHD 已属于新位置——后验把“source_path 匹配”当成成功，漏判“VHD 物理位置漂移”。
- 最坏后果：两个位置都存在 vhdx，后续 `wsl --terminate`/`--shutdown` 在新旧位置间产生竞态，用户以为成功但实际跑的是某个影子副本，数据写入错位置。
- 为什么现有保护不足：§11 把“原位置不存在活动 vhdx”列入检查但没有给出“如何在不删除、不触碰 vhdx 的前提下判定它已不被注册”——这在官方语义下本身就是“do not access”的高风险操作。
- 可验证修改建议：
  1. 后验改为**只读地**重新读 Lxss `BasePath` 并比较是否等于目标目录（验注册表归属，不动文件）；同时 `wsl -l -v` 确认状态为 Stopped→Starting→Running；再 + 8 秒等待后做 Bridge 健康校验。
  2. 把“原位置 vhdx 是否残留”降级为**信息项而非通过条件**：仅在成功迁移后由用户决定是否清理，不作为自动行为，避免触犯“do not access AppData WSL files”警告。
  3. 在 §11 明确引用 wsl-config“8 second rule”，要求重启 Bridge/Claude Science 前先 `--terminate <Distro>` 并等待≥8 秒。
- 对应测试/验收：309 上跑一次真实 Move，记录后验捕获顺序与 8 秒等待是否误报成功。

---

## 4. P1 发现

### P1-1 空间估算以 `VHDX逻辑大小` 为基准混淆了“逻辑 1TB 上限 / 物理占用 / 导出 tar 大小”，1.2× 与 +15GB 规则在最坏情况下不足

- 严重级别：P1
- 证据：方案 §5 目标余量 `>= max(VHDX逻辑大小 * 1.2, VHDX逻辑大小 + 15 GB)`；§5 还要求“分别计算迁移目标和备份空间，不重复计算”。但 disk-space 页明确 WSL2 VHD **逻辑上限默认 1TB**，物理占用远低于 1TB；“VHDX逻辑大小”若指 `Get-Item ext4.vhdx Length`（即物理占用），与 tar 导出文件大小（接近已用容量）又不是同量级；现有 `windows_storage_snapshot`（lib.rs 547）用的也是 `vhdxItem.Length`（物理）。
- 触发条件：用户发行版物理占用小但逻辑数据被 export 为 tar（接近已用量），或跨卷 Move 时目标盘仅按 1.2× 物理占准备但因 sparse 压缩阈值、`e2fsck` 临时文件等实际峰值更高。
- 最坏后果：相信“空间够”，迁移中盘写满，最坏破坏迁移一致性或退化为只读。
- 为什么现有保护不足：§5 没有区分“Move 目标空间 = max(物理占用×1.2, 已用×1.1)”与“备份空间 = tar ≈ 已用量”；没有要求把两者**分别**在目标盘/Backup 盘上各算一次峰值。
- 可验证修改建议：在 §5 给出三类独立空间阈值并显式标注来源（物理 `Length`、`df` 已用、tar≈已用），最坏情况取峰值×1.2；目标盘与备份盘各自满足；并要求目标盘文件系统类型检查（见 P1-4）。
- 对应测试：构造“物理小/逻辑满”临时发行版，验证阈值报红。

### P1-2 源盘低于 1GB 才挡执行过晚；备份/TEMP/WSL 命令仍可能再次写爆 C 盘

- 严重级别：P1
- 证据：方案 §5“源盘低于 1 GB、Windows 临时目录不可写或重启待办时不进入迁移执行”。但迁移执行阶段会 `wsl --shutdown`/`--terminate`、`--export` 到备份位、`--manage --move`——这些命令本身可能在 C 盘产生 TEMP 与 swap 写入（`.wslconfig` 默认 `swapFile = %Temp%\swap.vhdx`，wsl-config 页）。
- 触发条件：C 盘只剩 ~1GB，备份虽写到 D 盘，但 WSL/系统在 C 盘 TEMP 扩展临时占用，触发写爆，连导出都无法完成。
- 最坏后果：迁移中途 C 盘满，导出失败但已部分停机，状态卡死，且 C 盘更紧张。
- 为什么现有保护不足：1GB 门槛是同一把尺量了完全不同的写入压力。
- 可验证修改建议：把门槛分级——执行官方 Move 至少留 `max(2GB, VHD物理占用×0.05)`；执行 export-备份至少留足临时缓冲；执行前显式检查 C 盘 TEMP 所在盘与 swap.vhdx 所在盘是否同盘。
- 对应测试：低 C 盘压力注入，确认入口在门槛处拒绝给出清理建议而非继续。

### P1-3 快速无完整备份模式在官方 Move 语义未明的机器上 = 单边操作，断电/磁盘异常后无回滚，且方案把它定为“仅在官方Move路径提供”等于把恢复力绑定到一个未验证命令上

- 严重级别：P1
- 证据：方案 §7“快速迁移：仅在官方Move路径提供……恢复能力降低”。
- 触发条件：用户选了快速模式，Move 途中断电/目标盘掉线；因无完整导出备份，且 `--move` 失败后原发行版是否完好官方未保证。
- 最坏后果：发行版既不在新位置可启动、原位置也处于半移动状态，且无备份可救。
- 为什么现有保护不足：方案把“快速模式”局限在官方 Move，看似安全，但 P0-1 已证明官方 Move 语义本身未引用确认，绑死恢复力于该命令不可接受。
- 可验证修改建议：v0.1.4 **删除“快速无完整备份”模式**（与 Prompt 必答问题 2 的推荐一致），或在外部审查通过 + 309 实测官方 Move 失败可恢复性之前，默认关闭并标“实验/暂不提供”。
- 对应测试：注入断电模拟，验证选了快速模式被拒。

### P1-4 目标盘文件系统（NTFS / ReFS / exFAT/FAT32）未强制校验，存在“迁过去之后 Bridge/Claude Science 跑不起来”的伪成功

- 严重级别：P1
- 证据：方案 §5 排除了网络盘/可移动盘，但未要求校验目标盘文件系统类型；§16 待确认第 3 条“是否只推荐NTFS还是支持ReFS”。`--import-in-place` 官方明确“vhdx must be ext4 filesystem type”（basic-commands），但这是目标 vhdx 内部格式，外层 Windows 文件系统若为 exFAT/FAT32 可能不支持 reparse point、ACL 或稀疏属性，影响 WSL 注册语义。
- 触发条件：用户目标盘是 exFAT（常见于大容量外置盘双格式场景），Move 生成文件在 exFAT 上，ACL/ms0600 类语义中断（虽然 0600 是 Linux 侧 ext4 内部）。
- 最坏后果：迁移命令成功但运行时异常，与“桥接打不开”同构。
- 为什么现有保护不足：方案未要求枚举目标盘 `DriveFormat`。
- 可验证修改建议：§5 增加目标盘文件系统白名单默认为 **NTFS**；ReFS 标“需 309/VM 实测”；exFAT/FAT32 直接拒绝并给出解释；引用依据为官方对 vhdx 内部 ext4 的要求与 disk-space 文档的稀疏/属性相关说明。
- 对应测试：用 exFAT 目标盘注入，确认被拒绝。

### P1-5 计划指纹 TOCTOU：方案要求“执行前后检查计划指纹”，但未定义指纹字段集合与重检时点，存在“计划生成后环境变化但旧计划仍被执行”

- 严重级别：P1
- 证据：方案 §8“执行器在每个变更阶段前重新检测关键事实；发行版GUID/BasePath/VHDX/目标盘或计划指纹变化时拒绝继续”；§13“执行前后检查计划指纹，降低TOCTOU”。但未列出指纹具体字段，也未定义每个破坏性阶段前一次“指纹重算”。
- 触发条件：plan 生成后用户在另一窗口手动 `wsl --import`/`--unregister`/移走 vhdx，`status.json` 未变，旧 plan 仍被执行。
- 最坏后果：迁移/操作错误的发行版或对已被破坏的 BasePath 下手。
- 为什么现有保护不足：方案给了原则但未给字段与时点，等于未约束。
- 可验证修改建议（直接回答必答问题 6）：指纹至少含 `distroGUID + BasePath + vhdx sha256-of-stat(size+mtime+ino) + 目标盘卷序列号 + wsl_version + move_capability + generatedAt`；每个破坏性阶段（shutdown、backup、move、postcheck）进入前重算并比对 `expiresAt`（建议≤24h）；任一字段漂移即进入 `failed_original_intact` 并停止。
- 对应测试：plan 生成后篡改 BasePath，验证阶段入口拒绝。

### P1-6 默认用户变 root 与 systemd 状态恢复没有官方保证，import 路径下 `config --default-user` 不可用，方案未把恢复手段钉死

- 严重级别：P1
- 证据：方案 §6.2/§11 要求“默认Linux用户未变成root”“systemd状态符合迁移前”；basic-commands 页明确“`<Distro>config --default-user` will not work for imported distributions, because these distributions do not have an executable launcher. … use the `/etc/wsl.conf`”。
- 触发条件：兼容路径 import 后默认用户退回 root；systemd 因 `/etc/wsl.conf` 未恢复而关闭；之后 Bridge/Claude Science 以 root 起进程、配置权限 0600 失校。
- 最坏后果：迁移“成功”但 CSA 链路以错误用户与错误 systemd 状态运行，与 v0.1.0“换机桥接打不开”同构。
- 为什么现有保护不足：方案要求“记录 /etc/wsl.conf 与默认用户”，但没规定“import 后须显式恢复 /etc/wsl.conf `[user] default=` 并以 `wsl --terminate` + 8 秒等待重启生效”。
- 可验证修改建议：§6.2/§11 增加显式步骤：(1) 迁移前落盘 `/etc/wsl.conf` 完整内容、`wsl --version`、`systemctl --user` 状态、默认用户；(2) import 后写回 `/etc/wsl.conf` 并 `--terminate` + 等待生效；(3) 后验用实际登录用户（`id -un`）与记录比对，不一致即 `failed_recovery_required`。
- 对应测试：临时发行版 import 后默认用户变 root，验证检测拦截。

### P1-7 路径与命令安全：方案要求“结构化参数传递不拼接未转义 shell 字符串”，但现有 `windows_storage_snapshot` 与多处 PowerShell 以 `.replace()` 做参数插值，是 WSL/Distro 注入面

- 严重级别：P1
- 证据：方案 §13 要求不可信输入与不拼接；但 `lib.rs::windows_storage_snapshot`（518–559）用 `.replace("__DISTRO__", &distro)` 与 `'\'','#39;'` 转义生成 PowerShell 脚本；`legacy_windows_bridge_pid` 同样 `format!` 注入 `$escaped_root`。发行版名与路径是来自 Lxss/用户选择的不可信输入面。
- 触发条件：发行版名或目标路径含 `'`、`}`、`;`、`$(` 等构造，使插值后 PowerShell 执行额外语句。
- 最坏后果：路径/发行版名注入导致迁移脚本误删/误移/外泄。
- 为什么现有保护不足：只用单引号转义不够（`$(...)` 在双引号上下文、here-string 等场景仍可能注入）。
- 可验证修改建议：v0.1.4 的新迁移脚本统一改用 **PowerShell 参数数组传参**（`-ArgumentList`/`$args`/`param()`），不在脚本体内做字符串插值；发行版名做白名单正则（`^[A-Za-z0-9._-]+$`）；路径走 `Resolve-Path` 后逐段校验 reparse point（见 P1-8）。
- 对应测试：用含 `';Remove-Item ...` 的畸形发行版名/路径注入，验证被拒绝。

### P1-8 junction/symlink/reparse point 校验在方案里仅写了一句“解析最终绝对路径”，未规定拒绝/告警级别与处理方式

- 严重级别：P1
- 证据：方案 §13“验证目标路径解析后的绝对路径，防止junction/symlink和路径穿越”。现有 `Normalize-LocalPath`（lib.rs/status-probe.ps1）只剥离 `\\?\`，不展开 reparse point。
- 触发条件：用户把目标目录设成指向 C 盘某个 junction（指向源 BasePath 自身或其父目录），Move 把 VHD 写回源位置 → 无意义迁移；或指向 `\??\` 路径穿越。
- 最坏后果：迁移到错误位置、源目同盘循环写、甚至破坏源 VHD。
- 为什么现有保护不足：没有“展开 reparse point 后比对，禁止目标 ∈ 源 BasePath 子树”的硬校验。
- 可验证修改建议：用 `[System.IO.Path]::GetFullPath` + `fsutil reparsepoint query` / `[IO.Directory]` ResolveLinkTarget 展开链接后比对；目标解析路径不得等于源 BasePath 或其子目录；含 reparse point 默认拒绝并要求用户手动改选。
- 对应测试：目标目录建 junction 指向源 BasePath，验证拒绝。

### P1-9 迁移锁的位置与跨用户/跨 Agent 行为未定义，存在两个 Windows 用户或两个 Agent 并发的可能

- 严重级别：P1
- 证据：方案 §10 “同一发行版同一时间只允许一个迁移锁”，但未定义锁载体（文件？注册表？WSL 状态？）、锁作用域（per-user / per-machine）、以及跨 Windows 用户如何不互相踩。
- 触发条件：用户 A 已在迁移，用户 B 在另一会话也启动 CSA 并执行启动/Provider 切换；或两个 Codex Agent 同时被指派同一发行版。
- 最坏后果：双写导致状态机错乱、VHD 双副本/竞态。
- 为什么现有保护不足：方案给了原则缺了载体与作用域。
- 可验证修改建议（直接回答必答问题 7）：锁文件放 `%ProgramData%\ClaudeScienceAssistant\migration.lock`（machine-scope，跨用户互斥），内容含 `planId + distroGUID + pid + windowsUser + createdAt`；抢锁原子（`[IO.File]::Open(... FileShare None)`）；普通启动/升级在拿不到锁时只读提示“迁移进行中：操作者=X，请稍候或在其会话内取消”，不强制接管。
- 对应测试：双用户同时抢锁，验证后启动者收到明确提示而非静默踩。

---

## 5. P2/P3 发现

- **P2-1 产品边界命名值得保留但需加“升级 vs 迁移”说明**（§1）。方案已主动弃用“无风险迁移/一键修复”等命名，方向正确。建议在 UI 增加“这是迁移整个发行版，而不是升级 CSA；升级请用‘从旧版升级’流程”的明确分轨提示，防止与用户“升级”诉求混淆。P3 级交互改进。
- **P2-2 状态机列出了 18 个状态但缺“安全取消/回退”与“expired plan”分支**（§10）。建议补 `plan_expired`、`canceled_by_user`，二者均不应遗留迁移锁。
- **P2-3 “预计停机”标注为估算**（§4.2）合理，但未声明停机期间 Bridge/Claude Science 不可访问、用户正在跑的会话会被中断，建议加显式提醒。
- **P2-4 plan.json `secretsIncluded=false`**（§8）与现有 `secrets.values_included=false`（status-probe.ps1 / inspect *)一致，方向正确；建议再加一条“plan.json 不含 `wsl.conf` 全文，仅含 key 摘要”，因为 `wsl.conf` 可能含 `[boot] command` 等敏感本地命令。
- **P2-5 测试矩阵未列“两个 CSA 便携包升级接管”与“迁移中放弃后回到升级”两个剧本**（§15）。建议补入，直接对应 P0-4。
- **P3-1 文案**：“兼容Export-Import路径（高风险后备）”可改“高风险、仅生成方案、不自动执行”，更贴 v0.1.4 立场。
- **P3-2 UI**：`storageDetail`（App.tsx 649）已展示物理路径 + 宿主盘 + Linux 余量，建议补一句“该位置是 WSL 虚拟磁盘，迁移会搬动整个发行版”，与 P2-1 呼应。

---

## 6. 需求与风险追踪表

| 方案要求 | 风险 | 当前保护 | 缺口 | 建议 |
| --- | --- | --- | --- | --- |
| §1 不做一键无备份迁移 | 用户把“辅助迁移”当无风险 | 主动弃用风险命名 | 未区分“升级”与“迁移” | P2-1：UI 分轨说明 |
| §4.1 拒绝 WSL1/BasePath 不可解析 | 未拒绝“本机无 `--move` 能力” | 仅散列若干拒绝条件 | P0-3：缺能力不足分支 | 显式降级为只读诊断 + 人工指引 |
| §5 目标余量 1.2× / +15GB | 逻辑/物理/tar 混淆；C 盘 TEMP 写爆 | 分别算目标与备份 | P1-1/P1-2：分级阈值未定义 | 分三类空间阈值 + C 盘 TEMP/swap 检查 |
| §5 排除网络/可移动盘 | 未限文件系统类型 | — | P1-4：exFAT/FAT32 可通过 | 默认仅 NTFS，ReFS 待实测 |
| §6.1 官方 Move 优先 | `--move` 官方语义未引用 | 失败不自动 unregister | P0-1：地基缺失 | 引用门槛 + 假设标注 + 309 实测 |
| §6.2 兼容路径先备份再 unregister | 备份“可用性”未验证 | 校验大小/可读取 | P0-2：临时导入验证备份 | 临时导入副本验证后再 unregister |
| §7 快速无备份模式 | 绑死恢复力于未验证 Move | 仅官方 Move 才提供 | P1-3：删或默认关闭 | v0.1.4 关闭快速模式 |
| §8 计划指纹 | 字段与时点未定义 | 原则提及 | P1-5：TOCTOU | 指定字段集合 + 阶段入口重算 |
| §10 迁移锁 | 载体/作用域未定义 | 同一发行版一锁 | P1-9：跨用户 | machine-scope 锁文件 + 抢锁原子 |
| §10 状态机 | 缺取消/expired 分支 | 18 状态 | P2-2 | 补 `canceled_by_user`/`plan_expired` |
| §11 默认用户不变/systemd 不变 | import 后无官方保证 | 记录 | P1-6：恢复手段未钉死 | 显式写回 `/etc/wsl.conf` + `--terminate` 8s |
| §11 原位置无活动 vhdx | 触官方“do not access” | 列入检查 | P0-5：判据/8 秒未锚定 | 改为只读读 BasePath + 8 秒等待 |
| §13 路径/命令安全 | 现有 `.replace()` 注入面 | 原则提及 | P1-7/P1-8 | 参数数组 + 白名单 + reparse 展开 |
| §16 待确认第1项“只做生成方案+Codex执行” | 升级路径被迁移状态阻塞 | 未提升级 | P0-4：升级/迁移互斥 | 分 A/B 双路径 + 安全取消出口 |
| §15 测试矩阵 | 缺“无能力机型”与“升级接管”剧本 | 列出环境 | P0-3/P2-5 | 补两个必测剧本 |

---

## 7. 迁移方式评审

### 7.1 官方 `--manage --move`

- **状态：可作为候选主路径，但官方语义未被确认，Build 前必须以 309/VM 实测坐实保留内容与失败语义（P0-1）。**
- 优点：若官方实现等价于“原地保留发行版身份、仅改 VHD 物理位置”，则天然避免 import/unregister/name-loss；与方案“不自动 unregister”契合。
- 风险：(1) `--move` 在 basic-commands/disk-space 两份官方文档无条目，仅见 WSL repo 内部 `MoveDistribution` bug 历史（2.7.3、2.9.3），说明跨卷/AUTHORITY 处理在历史版本有问题；(2) `--manage` 整体门槛 WSL 2.5+，对 Win10 19045 inbox WSL 不适用；(3) sparse 属性只对新建 VHD 生效（wsl-config 页），move 来的 vhdx 不会自动稀疏化，空间收益可能低于预期。
- 结论：v0.1.4 可提供“若 `wsl --help` 显式声明 `--manage ... --move` 且 `wsl --version` ≥ 阈值则推荐”，但必须以 309 实测为通过门槛，否则降级。

### 7.2 Export-Import

- **状态：高风险，仅生成方案，绝不内置执行器，且 unregister 前强制“临时导入验证备份”（P0-2）。**
- 关键官方事实：`--unregister` 不可逆全丢（basic-commands）；`--import` 产生新发行版身份（默认用户/默认发行版/UUID 不自动保留）；导入发行版无 launcher，`config --default-user` 不适用，须改 `/etc/wsl.conf`（basic-commands）。
- 结论：v0.1.4 仅生成兼容方案 + 恢复命令模板，全部破坏性步骤列为永不自动执行；v0.2 再评估执行器且需经得起单独安全评审。

### 7.3 Import-in-place

- **状态：可用于“Move 成功但原位置残留副本需重建归属”的恢复，且可用于“备份验证 import 副本”（P0-2/P1-6）；但目标 vhdx 必须 ext4 文件系统（basic-commands 明文）。**
- 注意：`--import-in-place` 同样以“new distribution”身份导入，名称/GUID 与原不同，不能等价于原地恢复；作恢复路径时需用**临时名**导入并通过后验，避免与原发行版混淆。
- 结论：作为只读诊断之外的恢复工具候选，但不作为 v0.1.4 默认执行路径。

---

## 8. 状态机与恢复评审

- 18 状态覆盖了正常与故障主干，但**缺两个安全出口**：`canceled_by_user`（用户中途放弃）与 `plan_expired`（指纹/时间过期）——二者必须能无副作用回到 `eligible`/`not_needed` 且释放迁移锁（P2-2）。
- “命令成功但状态文件没写” vs “状态文件已写但命令未成功”：方案 §9 要求每阶段写 `status.json` 再验证，方向正确；但需补“写 `status.json` 用临时文件 + 原子 rename”，避免崩溃中写到一半的状态文件被当作可信（与现有 key 切换原子提交一致，见 v0.1-current-pc-verification 第2节 Key 切换事务）。
- `failed_original_intact` 与 `failed_recovery_required` 区分合理；但需明确判定“原发行版仍注册且可启动”采用只读证据（`wsl -l -v` 状态 + 一次 `--terminate`/启动尝试），不得触碰 vhdx（P0-5）。
- 跨崩溃/重启恢复：方案 §16/v0.2 列入，但 v0.1.4 必须至少保证“崩溃后下次打开 CSA 能读到未完成迁移并提示用户在某会话内继续或取消”，而不是静默继续或静默丢锁（P0-4）。

---

## 9. 路径、权限与命令安全评审

- **不可信输入面**：方案已把发行版名/用户名/卷标签列为不可信，方向正确。但落地在 v0.1.4 脚本时必须避免现有 `lib.rs` 与 PowerShell 里的 `.replace/__DISTRO__/format!` 字符串插值模式（P1-7），改用参数数组。
- **路径解析**：必须由 `Resolve-Path` + reparse point 展开 + 与源 BasePath 子树比对（P1-8）；现状 `Normalize-LocalPath` 只剥 `\\?\`，不足以防 junction 注入。
- **删除/Move/备份目标证明**：方案 §13“任何 fallback 不得扩大操作范围”正确；建议把“只操作用户明确选择的一个发行版”从文案升级为代码级断言：在每条破坏性命令前后断言目标路径 ∈ 用户已确认集合。
- **进程停止非宽名**：与 `AGENTS.md`“Do not stop a process by a broad name”一致；`start-claude-science-wsl.sh::stop_stale_bridge_listener` 已按 `cmdline` 含 `/proxy.py` 精确匹配，迁移脚本应沿用同一判据而非另起宽匹配。
- **不动 Lxss 注册表**：方案明确“不以修改 Lxss 注册表代替官方命令”，与官方 disk-space“do not modify/move WSL AppData files”一致；保留此约束。

---

## 10. 空间、备份与回滚评审

- 空间见 P1-1/P1-2。结论：方案应给三类独立空间阈值（Move 目标 / backup 目标 / C 盘 TEMP&swap 缓冲），并分别验证。
- 备份“可恢复”判据必须从“文件存在+大小合理”升到“临时导入副本可启动”（P0-2）。
- 快速无备份模式在官方 Move 语义坐实前应关闭（P1-3）。
- 回滚“反向 Move/恢复 import 需要源盘空间”：方案 §12 提到“保留导出文件、在恢复名下导入”，但未评估“反向 Move 又需一份目标盘空间”，需在恢复计划里显式列出反向空间需求，避免回滚也卡空间。
- 备份与目标“不在同一物理故障域”：方案 §7 已要求，但未说明如何判定（同一物理磁盘不同卷仍是同一域）。建议用卷序列号/物理盘索引判，而非盘符。

---

## 11. 测试矩阵缺口

应补足以下剧本，每个都要能在临时发行版/VM 上跑而不碰用户真实发行版：

| # | 缺失剧本 | 关联发现 |
| --- | --- | --- |
| T1 | Win10 19045 inbox WSL：`wsl --help` 无 `--manage`/`--move`，入口应降级而非误报 | P0-3 |
| T2 | 用户生成迁移 plan 后中途放弃 → 双击新版 exe 升级 → 升级可完成 | P0-4 |
| T3 | 两个 Windows 用户会话同时操作同一发行版 → 锁互斥 + 明确提示 | P1-9 |
| T4 | 残缺 export 文件 → 临时导入验证拦截 unregister | P0-2 |
| T5 | 目标盘 exFAT/FAT32 → 入口拒绝 | P1-4 |
| T6 | 目标目录是 junction 指向源 BasePath → 入口拒绝 | P1-8 |
| T7 | 发行版名/路径含 `'` 与 `$(...)` → 不执行额外指令 | P1-7 |
| T8 | C 盘仅 ~1–2GB，备份写到 D → 入口在执行门槛处拒绝 | P1-2 |
| T9 | Move 后默认用户变 root / systemd 关 → 后验拦截 | P1-6 |
| T10 | plan 生成后 BasePath/vhdx 被外部篡改 → 阶段入口拒绝 | P1-5 |
| T11 | 崩溃在 `move_in_progress` → 重开 CSA 读未完成迁移并提示 | P0-4 |
| T12 | 两份升级/接管并存 → 旧目录 Bridge 被新目录精确接管、不双实例 | P2-5/P0-4 |

---

## 12. 对10个必答问题的逐项回答

1. **v0.1.4 只做“生成方案+Codex执行”是否足够安全？**
   不充分。方向正确（不内置自动执行减小破坏面），但必须同时落地“升级路径不被迁移状态阻塞”（P0-4）、“能力不足机型降级”（P0-3）、“`--move` 语义未明确即不作为默认主路径”（P0-1），否则“生成方案”会在不安全前提下生成看似可执行的 plan。

2. **是否应该完全删除“快速无完整备份”模式？**
   建议在 v0.1.4 默认关闭/删除。在 309 实测官方 Move 失败可恢复性并坐实保留语义前，快速模式等于单边操作无回滚（P1-3）。

3. **官方 Move 前是否必须导出备份；如果不是，最低安全门槛是什么？**
   官方未强制，但本方案不应低于“最低安全门槛 = Move 命令成功且目标 vhdx 可读 + 发行版可启动 + 默认用户/systemd 不变 + 原 BasePath 注册归属已迁移（只读读 Lxss）+ 原 vhdx 不被触碰”。若达不成该门槛，则必须补完整可验证备份（P0-1/P0-5）。

4. **兼容 Export-Import 路径是否应推迟到 v0.2 或更晚？**
   推迟到 v0.2（仅“生成方案/恢复命令模板”留在 v0.1.4，不内置执行器）。自动执行器所需的“临时导入验证备份 + 永不自动 unregister + 跨机失败注入”需单独评审，v0.1.4 不承载（P0-2）。

5. **目标盘应仅限NTFS，还是可以支持ReFS；依据是什么？**
   v0.1.4 默认仅 NTFS。依据：官方 `--import-in-place` 明确 vhdx 需 ext4 文件系统，外层 Windows 文件系统对 reparse/ACL/稀疏/recovery 的支持直接影响 WSL 注册语义；ReFS 在 CSA 覆盖范围内未被测过，列为 309/VM 实验项后视情放开（P1-4）。

6. **计划指纹应包含哪些字段，如何避免TOCTOU？**
   见 P1-5。最少：`distroGUID + BasePath(resolved) + vhdx stat(size+mtime+ino) + 目标卷序列号 + 目标resolved路径 + wsl_version + move_capability + generatedAt + expiresAt(≤24h)`；每个破坏性阶段入口重算并比对，漂移即停止；同时阶段内前后双向校验（P0-5）。

7. **迁移锁放在哪里，如何处理两个Windows用户或两个Agent？**
   见 P1-9。machine-scope 锁文件 `%ProgramData%\ClaudeScienceAssistant\migration.lock`，内容 `planId/distroGUID/pid/windowsUser/createdAt`；原子独占打开；抢不到只读提示操作者，不强制接管；进程退出/会话取消必须释放（响应 P2-2 的取消分支）。

8. **什么情况下必须拒绝迁移，而不是继续给出警告？**
   必须拒绝（入口不生成可执行 plan）：(a) 本机无 `--manage/--move` 能力且无经过 309 验证的兼容执行器；(b) WSL1；(c) BasePath/vhdx 不可定位或发行版注册不一致；(d) 根只读/`/tmp`/home 不可写；(e) 目标盘文件系统非 NTFS 或为网络/可移动盘；(f) 源盘低于执行门槛缓冲（P1-2）；(g) 目标 ∈ 源 BasePath 子树或含 reparse；(h) 发行版名/路径未通过白名单；(i) 存在同发行版未释放的迁移锁（P0-3/P1-2/P1-4/P1-8/P1-9）。

9. **迁移成功的最小充分证据是什么？**
   退出码 0 **不充分**。最小充分证据集（均只读）：`wsl -l -v <Distro>` 状态 Running→目标目录 + 重新读 Lxss `BasePath`=目标 + 目标 ext4.vhdx 存在可读 + 实际登录用户=`id -un` 等于迁移前记录 + systemd 状态(`ps -p1`=systemd / `systemctl --user` 状态)与记录一致 + Bridge `/health` 200 且 `source_path` 属当前 CSA 包且 `config_revision` 一致 + 端口 8765/8766 可达 + `/tmp`、home 可写 + 配置 `0600` + C 盘余量较迁移前增长 + 原 BasePath 仅残留（非强制清理）。任一不满足落于 `failed_recovery_required`（P0-5/P1-6/§11）。

10. **哪些未知事项需要在309或临时VM上做实验才能决定？**
    (1) `--manage --move` 的保留内容（名/GUID/默认用户/默认发行版/WSL版本/sparse）与失败/半移动语义，跨卷 C→D/E/H 在 Win10 19045 与 Win11 Store 各跑一次（P0-1）；
    (2) 无 `--manage` 的 inbox WSL（309）上入口降级行为（P0-3）；
    (3) ReFS 目标盘是否可正常承载并运行（P1-4）；
    (4) 兼容路径“临时导入验证备份”的可行性与失败注入（P0-2）；
    (5) 跨用户迁移锁互斥（P1-9）；
    (6) Move 后默认用户变 root / systemd 关闭的真实触发条件与恢复手段（P1-6）；
    (7) 崩溃在 `move_in_progress` 后重新打开 CSA 的恢复提示行（P0-4）。

---

## 13. 建议修改稿

以下为可直接替换/新增进方案的文本（保持方案既有章节编号）：

### 13.1 在 §1 目标与边界末尾新增“两条产品路径互斥”段落

> CSA 区分两条互斥的产品路径：
> - **（A）就地升级**：同发行版、同 VHD，仅替换 CSA 便携包目录并由新版接管旧目录的 Bridge（见 README“从旧版升级”）。这是普通升级路径，不涉及存储迁移。
> - **（B）存储迁移**：物理搬迁整个发行版 VHD 到其他盘。
> 两条路径互斥：迁移锁存在时拒绝执行升级接管；但必须提供“取消迁移并安全回到就地升级”的一键出口，取消时仅清理迁移私有状态、不删除任何备份、不动 VHD。“辅助迁移”入口必须可忽略、不阻塞启动或升级。

### 13.2 替换 §6 首段（特性检测）

> 不要仅按版本号猜测能力。特性检测分两层：
> 1. 以当前机器 `wsl.exe --help` 是否声明 `--manage <Distro> --move <Location>` 作为主要特性门；同时记录 `wsl --version` 与 WSL 安装通道（Store / inbox），并按官方文档 `--manage` 整体门槛要求 `version >= 2.5`。
> 2. 当 `--help` 不含 `--manage` 或不含 `--move` 子项时，入口降级为**只读诊断 + 人工迁移指引 + 建议升级 WSL**，并在 `plan.json` 写入 `migration_method = unsupported`，不生成可执行迁移 plan。

### 13.3 在 §6.1 官方 Move 路径前补“假设标注”

> 官方 Microsoft Learn basic-commands 与 disk-space 两份文档截至访问日（2026-07-11）未含 `--move` 条目；`--manage` 整体明文门槛为“WSL releases 2.5 and higher”。因此本方案对 `--manage --move` 采取以下**假设（非官方保证）**：Move 等价于“发行版身份、GUID、默认用户、默认发行版状态、WSL 版本原地保留，仅改 VHD 物理位置”，并要求先 `wsl --terminate <Distro>` 后等待≥8 秒再迁移。该假设必须在 309 / 临时 VM 上以临时测试发行版实测坐实，否则 `--move` 不作为首版默认主路径。

### 13.4 在 §6.2 兼容路径“先验证导出文件存在、大小合理、可读取”处替换为

> 在对原发行版执行 `wsl --unregister` 之前，必须完成**不可旁路的恢复可行性验证**：使用导出文件以**临时发行版名 + 临时目录**执行一次 `wsl --import` 或 `--import-in-place`，并验证该副本可启动、默认用户正确、VHD 可挂载。只有验证副本通过，才允许对原发行版执行 unregister。`wsl --unregister` 在 v0.1.4 永不自动执行。

### 13.5 在 §5 目标盘选择规则补充

> - 目标盘文件系统必须为 **NTFS**（ReFS 需 309/VM 实测后放开；exFAT/FAT32/未知一律拒绝）。
> - 目标解析路径展开 reparse point 后，不得等于源 `BasePath` 或其子目录。
> - 空间阈值分三类独立计算：① Move 目标 `max(VHDX物理占用×1.2, 已用量×1.1)`；② 备份目标 `tar≈已用量` 并单独留 `已用量×0.1` 缓冲；③ C 盘 TEMP/swap 缓冲 `max(2GB, VHD物理占用×0.05)`。三类各自在对应盘满足方可进入执行。

### 13.6 在 §10 状态机补状态与锁载体

> 新增状态：`canceled_by_user`、`plan_expired`，二者无副作用回到 `eligible`/`not_needed` 且释放迁移锁。`status.json` 写入采用临时文件 + 原子 rename。
> 迁移锁：machine-scope，路径 `%ProgramData%\ClaudeScienceAssistant\migration.lock`，内容含 `planId + distroGUID + pid + windowsUser + createdAt`，以独占方式原子打开；抢不到者只读提示操作者，不强制接管。

### 13.7 对 §11 后验检查替换“原位置不存在仍被注册使用的活动 VHDX”

> 该项降级为**信息项**：迁移成功后约用户清理原位置残留，不自动处理，避免触犯官方“do not access WSL AppData files”警告。成功判据只读地重新读 Lxss `BasePath` 等于目标目录、`wsl -l -v` 状态 Running、`id -un` 等于迁移前记录，并依官方“8 second rule”在 `--terminate` 后等待≥8 秒再做 Bridge 健康校验。

### 13.8 对 §7 快速迁移模式的处理

> v0.1.4 默认不提供“快速无完整备份”模式，待 309 实测官方 Move 失败可恢复性并坐实保留语义后再以开关形式评估。

---

## 14. Build前清单

- [ ] 所有 P0 关闭（P0-1 `--move` 语义引用/假设标注/309 实测前不作为默认主路径；P0-2 unregister 前强制临时导入验证备份；P0-3 无能力机型明确降级；P0-4 升级/迁移互斥 + 安全取消出口；P0-5 成功判据只读 + 8 秒等待 + 不触碰 vhdx）
- [ ] 所有 P1 关闭或被用户明确接受（P1-1..P1-9）
- [ ] 官方 WSL 语义有引用：basic-commands / disk-space / wsl-config 三份 Microsoft Learn URL 与访问日期已写入方案
- [ ] 恢复路径可演练：用临时测试发行版演练“Move 失败→原发行版完整判定”“export 残缺→拦截 unregister”“崩溃在 move_in_progress→重开提示”
- [ ] 309/VM 实验项明确（§3 未能验证事实 1–4、§12 第10问 1–7）
- [ ] 跨机器泛化：测试矩阵含 T1（无能力机型）与 T12（双升级接管）两个剧本
- [ ] 升级路径：README“从旧版升级”与迁移方案互斥关系写入开发者说明，保证“新装/升级/迁移”三条用户起点均不会互相卡死

---

## 15. 最终建议

方案在“不做无确认无备份一刀切迁移、官方命令优先、不自动 unregister、计划指纹 + Codex 逐阶段确认”这一总体方向是安全且值得肯定的；Codex 执行边界（§9）与“不输出秘密/不改网络”（与 `AGENTS.md` 一致）也写得到位。

但作为通用产品的 Build 前方案，它目前达不到 GO，关键卡点集中在用户本轮强调的两点：

1. **泛化性（防 v0.1.0 重演）**：`--manage --move` 的官方语义未被引用，方案把“当前开发机能跑”当事实用；对无 `--manage/--move` 能力的 Win10 inbox WSL 没有明确产品分支。若不补齐，极可能在 309 与普通用户机上重演“换机桥接打不开/迁移按钮无意义”的 v0.1.0 教训（P0-1/P0-3）。
2. **升级路径操作性**：全新安装一直不是问题，真正风险是“已有发行版 + 想升级”的用户被推入“迁移整个发行版”的危险流程，并在迁移状态中途崩溃后同时被升级路径与迁移状态卡死。必须把“就地升级（A）”与“存储迁移（B）”显式互斥并给安全取消出口（P0-4）。

因此结论为 **CONDITIONAL APPROVE + NO-GO**：方案方向可批准，但在 P0-1~P0-5 关闭、`--move` 官方语义明确引用并经 309/VM 实测、以及升级/迁移双路径解耦完成前，不进入 Build。

最关键的三个修改点：
1. 引用 `--manage` 官方门槛与 `--move` 语义不确定性，把 `--move` 在 309 实测前降级，并补能力不足机型的明确降级分支（消化 P0-1/P0-3，直接回应泛化性诉求）。
2. 把“就地升级”与“存储迁移”显式互斥，并补“安全取消迁移→回到升级”的一键出口（消化 P0-4，直接回应升级路径诉求）。
3. 把 unregister 前的备份校验从“文件存在+大小合理”升级为“临时导入副本可启动”的不可旁路验证，并关闭快速无备份模式直到官方 Move 失败可恢复性被实测（消化 P0-2/P1-3）。

残余风险与未执行测试（即使上述三项全部完成）：`--move` 在跨卷/Win10/Win11 不同 WSL 通道下的真实保留与失败语义仍需 309 实测坐实；ReFS 目标盘、跨用户锁、崩溃中恢复提示行均需在临时发行版/VM 剧本里验证；本审查为只读 Review，未在真实发行版上运行任何迁移命令。
