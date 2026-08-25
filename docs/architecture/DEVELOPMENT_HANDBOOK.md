# CSA 开发手册

> 面向下一位接手 CSA 的开发者或模型。本文记录长期边界和可重复的开发方法；单轮需求与现场结果应写入 `docs/reports/` 和 `TASKS_PROGRESS.md`。

## 一、这个项目是什么（30 秒版）

CSA 是一个 Windows 控制面，管理唯一的 WSL Claude Science 运行时和当前包 Bridge；桌面 UI 负责启动、打开、配置 Key、观察和审批修复。深度体检、出口探针、存储迁移、运行时升级都是车间能力，不是点火前提。

所有能力只允许进入三条车道：

- **ALLOW（点火）**：只回答“现在能不能打开”。
- **GRADE（仪表）**：展示健康、质量和风险；失败可以报警，但不能扣住主按钮。
- **WORK（车间）**：用户明确触发的诊断、修复、升级或可能计费的动作；不得自动成为启动条件。

唯一的锁是 `canOpen = claudeRunning && !windowsBridgePid`。冻结输入集合严格为 `{claudeRunning, windowsBridgePid}`；能力可以增加，锁不能暗增。长期合同见 `docs/architecture/CURRENT_CONTRACT.md`，变更入口见 `docs/architecture/PR_CHECKLIST.md`。

### 两轮可复算基线

| 轮次 | Git 范围（前开后闭） | fix | feat | refactor | test | docs | 总计 | `git diff --stat` |
|---|---|---:|---:|---:|---:|---:|---:|---|
| v0.1.6 S0–S5 | `03dc6c2..c9ca823` | 2 | 2 | 2 | 1 | 3 | 10 | 24 files，+6303 / -352 |
| v0.1.7 T0–T6 | `c9ca823..8a7e12e` | 0 | 1 | 0 | 0 | 3 | 4 | 14 files，+3829 / -252 |

按 PowerShell 管道的非空行口径，v0.1.6 的 `lib.rs` 为 7086 → 8411（+1325），`App.tsx` 为 1883 → 2390（+507）；v0.1.7 分别为 8411 → 8876（+465）和 2390 → 2515（+125）。两轮合计 `lib.rs` +1790（+25.3%），`App.tsx` +632（+33.6%）。若按物理行（含空行）复算，对应快照为 `lib.rs` 7521 → 8919 → 9407，`App.tsx` 2001 → 2528 → 2654；比较时必须声明口径，不能混用。

## 二、做对了什么（要有证据，不许自夸）

### 1. 用冻结测试定义唯一的锁

- **做法**：把允许打开的输入集合和主按钮文案输入都焊进 `allow_inputs_frozen`，产品代码通过同一纯函数计算。
- **证据**：`c9266c5 refactor: freeze CSA allow status contract`；`launcher/src-tauri/src/lib.rs` 的 `allow_inputs_frozen`；v0.1.7 T5 仍断言集合严格等于 `{claudeRunning, windowsBridgePid}`。
- **为什么有效**：文档只能提醒，冻结测试会让任何偷偷把磁盘、出口或探针塞回主按钮的提交直接失败。
- **下次还这么做的条件**：凡是修改 ALLOW DTO、主按钮标签或 disabled 条件，先跑冻结测试；确需扩锁必须走合同税单，不能顺手改期望值。

### 2. 用“全红仍能打开”的反向场景防回归

- **做法**：同时构造出口 502、DrvFS D/p9、磁盘告警、代理冲突、identity 不匹配、canary/版本超时，反向断言主按钮不变。
- **证据**：`75eff77 test: enforce non-gating CSA probe lanes`；`launcher/tests/all-probes-red.test.ts` 的 `all_probes_red_button_still_open / bridge_egress_never_gates`。
- **为什么有效**：逐个成功用例证明不了组合故障；全红反例直接覆盖“多个非阻断状态被 AND 回门禁”的历史病型。
- **下次还这么做的条件**：新增任何 GRADE/WORK 状态时，把最坏失败态加入全红夹具，但不得改变主按钮期望值。

### 3. 用进程内 `--smoke` 代替打正式包调试

- **做法**：增加 debug `--smoke [--only ...]`，复用产品 ALLOW/open/Bridge/egress/grade 实现，不启动 GUI、不重启服务、不弹浏览器。
- **证据**：`b201d15 feat: add in-process CSA smoke loop`；`launcher/src-tauri/src/smoke.rs` 与 `scripts/csa-smoke.ps1`；v0.1.7 最终 smoke 6195 ms、required 4/4 PASS、exit 0。
- **为什么有效**：同源码产物在秒级给出结构化证据，避免“改一处、打一个约 90 MB ZIP、再手点”的分钟级循环和陈旧 EXE 假绿。
- **下次还这么做的条件**：先以独立 `CARGO_TARGET_DIR` 构建并跑 smoke；只有发布阶段才制作正式 ZIP。

### 4. 用分层错误码标明故障发生在哪条链路

- **做法**：登录、运行时、Bridge、仪表和车间错误分别使用 `login.*`、`runtime.*`、`bridge.*`、`grade.*`、`work.*`、`transport.*` 前缀；出口再细分代理连接与 401/403 认证。
- **证据**：`docs/architecture/CURRENT_CONTRACT.md` §6；v0.1.7 的 `preflight_error_attributes_layer`；验收报告记录 `work.bridge_egress.proxy_dead` 与认证失败的两套文案。
- **为什么有效**：10808 死代理不再被误报成“Key 错”，用户和开发者可以沿着真正失败的层排查。
- **下次还这么做的条件**：新增失败路径前先在合同表中找到归属；错误首行必须能回答“哪一层失败”，详情再承载底层信息。

### 5. 复用“只读体检 → 分层报告 → 修复 Prompt → 用户审批”

- **做法**：体检默认只读，把事实、结论和建议分开；需要写入时先生成完整 Prompt 或二次确认，由用户明确批准。
- **证据**：`launcher/src/storageMigration.ts`、`launcher/src/runtimeUpdate.ts`、`launcher/src/bridgeEgress.ts`；`7fe04c1 feat: add CSA Bridge egress work probe` 与 `6115f15 feat: add self-healing Bridge egress`。
- **为什么有效**：诊断不会为了证明问题而改机器；危险动作的目标、字段、备份、回滚和验证都能先被审阅。
- **下次还这么做的条件**：非 ALLOW 的新能力优先采用这一形状；只有用户确认后才进入有副作用或可能计费的步骤。

### 6. 把事务成功定义为“外部证据成立”，而不是函数返回

- **做法**：Provider 切换和出口修复在写入前后校验受管 Bridge owner/source、revision、监听 PID、配置读回和真实请求；失败只回滚目标字段。
- **证据**：v0.1.7 验收的 DeepSeek → OpenRouter → DeepSeek 两次真实切换；`apply_fix_is_partial_update`、`apply_fix_rolls_back_on_failure`；Claude PID 全程保持不变。
- **为什么有效**：UI 显示成功不再等同于实际生效；局部回滚不会覆盖并发产生的 Key、模型或路由变化。
- **下次还这么做的条件**：跨 Windows 设置、WSL 配置和 Bridge 进程的动作都必须给出 before/after 外证据，并明确崩溃窗口。

### 7. 区分 LIVE、PREVIEW、TEST 和待人工

- **做法**：验收报告明确标注真实点击、浏览器 fixture、自动化证明和未执行项目，不用源码存在冒充现场成功。
- **证据**：`docs/reports/CSA_v016_S5_capability_audit_20260825.md` 的证据口径与 A–E 逐项表；破坏性、计费和正式发布项仍诚实保留为待人工。
- **为什么有效**：避免 preview 假绿、隐藏副作用和“34 项全绿”式夸大；接手者知道哪些结论能在真机复现。
- **下次还这么做的条件**：每条验收必须带证据类别；涉及服务、凭据、额度或发布的动作没有授权就标待人工。

## 三、做错了什么 / 走过的弯路

### 1. 用发布包当调试器

- **当时怎么做的**：2026-08-24 一天制作 r2/r5/r6/r7/r8 五个正式 Release ZIP 来追启动问题。
- **代价是什么**：反馈周期被打包、解压和接管流程拉长，还引入“到底跑的是哪个目录/哪个 EXE”的来源歧义。
- **根因**：没有可复用产品路径的非 GUI 快速入口，把发布工序误当成运行时诊断工具。
- **现在的规矩**：日常验证只用独立目标目录的 debug build + `--smoke`；正式 ZIP 只在发布门执行一次。

### 2. 连续打补丁，没有先修结构

- **当时怎么做的**：以 `03dc6c2` 为开工点向前看最近 12 个提交（`be3005d` 至 `03dc6c2`）全部是 `fix:`，`refactor:` 和 `test:` 都为 0。
- **代价是什么**：同一种“启动不了/诊断阻断”表现反复出现，修复彼此叠加，系统越来越难推理。
- **根因**：按单次报错改分支条件，没有先冻结 ALLOW 并拆开 GRADE/WORK。
- **现在的规矩**：第三次出现同型故障前必须停下来画调用图、补反例和结构测试；禁止继续堆条件分支。

### 3. 台账断更，提交历史失去上下文

- **当时怎么做的**：`TASKS_PROGRESS.md` 曾停在 v0.1.5 的 120 行，后续 12 个 fix 没有一条记录。
- **代价是什么**：任务书的根因描述、现场真实状态和代码已经分叉，只能靠重新测绘还原。
- **根因**：把台账当交付收尾，而不是每一阶段的完成条件。
- **现在的规矩**：每个阶段提交前写“做了什么 / 证据在哪 / 下一步”三行；没有台账就不算完成。

### 4. 探针测了错误的链路

- **当时怎么做的**：大量检查 daemon 沙盒的 SOCKS/PyPI 路径，却没有检查用户请求实际经过的 Bridge `httpx(trust_env=false, outbound_proxy_url=...)`；S0 时 `lib.rs` 中 `outbound_proxy` 命中为 0。
- **代价是什么**：沙盒 3/3 和代理状态全绿时，Bridge 仍指向无人监听的 10808，所有模型请求返回 502，用户被引导去怀疑 Key。
- **根因**：从“有什么探针”出发，而不是从一次真实用户请求反向画端到端数据流。
- **现在的规矩**：新增可用性结论前必须画“用户动作 → 实际进程 → 实际配置 → 上游”的链路；探针必须与该链路共享权威实现。

### 5. 根因描述过期，差点按旧问题返工

- **当时怎么做的**：v0.1.6 任务书仍写“全部状态 AND 进主按钮”，但开工测绘发现 `canOpen` 已被前几个 fix 收敛为两个输入。
- **代价是什么**：如果直接按任务书重拆，会重复修改正确代码，并可能再次扩大门禁。
- **根因**：把需求文档的历史描述当成当前事实，没有先对 HEAD 做只读测绘。
- **现在的规矩**：任务开始先建立 commit 基线、调用图和反例；文档与源码冲突时记录差异，以可复算源码事实为准。

### 6. 单体文件持续膨胀

- **当时怎么做的**：即使新增了 `smoke.rs`、`bridge_egress.rs` 和 `bridge_egress_apply.rs`，两轮仍让 `lib.rs` 非空行 7086 → 8876（+25.3%），`App.tsx` 1883 → 2515（+33.6%）。
- **代价是什么**：审查面、编译反馈、冲突概率和“改外观误碰事务”的风险同时上升。
- **根因**：新模块只承接新增算法，旧 command、DTO、设置事务和大量测试仍集中在 `lib.rs`/`App.tsx`。
- **现在的规矩**：新增能力前先指定归属模块；稳定轮不得净增首页模块，单体文件增长必须在 diff 说明中解释并给出抽取计划。

### 7. 首屏和周期刷新曾让 GRADE 覆盖 ALLOW

- **当时怎么做的**：首屏调用 `initialize_runtime` 并置全局 busy，30 秒刷新用全量 `get_system_status` 整体覆盖状态。
- **代价是什么**：慢体检或瞬态 p9 I/O 会让已经可用的系统显示“不能打开”，用户陷入修复/刷新鬼打墙。
- **根因**：DTO、刷新时钟和 busy 状态没有按三车道隔离。
- **现在的规矩**：首屏只取 ALLOW；周期刷新只能更新自己的车道；GRADE/WORK 不得写 ALLOW 字段或主按钮 busy。

### 8. WSL 命令经过多层展开却按单层 shell 思考

- **当时怎么做的**：经 `wsl.exe` 传入 `--noproxy "*"`，在 DrvFS 当前目录被展开；此前还存在 `bash -lc` 多行和未保护 `$()` 的运输债。
- **代价是什么**：配置面板真实点击等待 8 秒后报 `bridge.dashboard_url_failed`，curl 把工作区文件名当 URL；同类问题还可能泄漏或误执行参数。
- **根因**：没有把 PowerShell/Windows argv/WSL shell/目标程序当作四层独立解析器。
- **现在的规矩**：固定 argv 用数组，多行脚本走 stdin → `bash -s`；直接 argv 禁止裸 glob，新增运输必须有“外层会展开”的反例测试。

### 9. 文案隐藏了动作的真实副作用

- **当时怎么做的**：页脚只写“重启”，实际还会备份并收窄持久 DrvFS 授权。
- **代价是什么**：用户无法在点击前判断影响范围，验收人员也可能在非维护窗口误触。
- **根因**：把短文案优先级放在可预期性之前，没有把 UI 入口与后端副作用逐项对照。
- **现在的规矩**：按钮名称、说明和确认层必须列出会改什么；有持久副作用的动作不得伪装成普通重启。

### 10. Preview 成功分支曾留下旧错误

- **当时怎么做的**：浏览器 preview 的保存、测试、映射、激活、删除和聚合成功后没有统一清除旧错误。
- **代价是什么**：界面会同时显示“已生效”和“未应用”，自动化若跨到 Tauri 分支还可能假绿。
- **根因**：成功状态和错误状态由多个分支分别维护，测试没有先截断真实后端分支。
- **现在的规矩**：事务状态使用唯一收口；成功必须清理同一操作的旧错误；preview 只能证明 UI wiring，不能当 LIVE 后端证据。

### 11. Provider 事务仍有崩溃一致性窗口

- **当时怎么做的**：正常错误路径有回滚和双 watchdog，但 Bridge 已提交、Windows settings 尚未提交之间没有跨进程 durable journal。
- **代价是什么**：若进程或机器在该窗口崩溃，可能出现 Bridge 使用新配置、UI 仍显示旧配置的分裂状态。
- **根因**：只设计了函数内失败回滚，没有为进程死亡后的恢复定义持久提交协议。
- **现在的规矩**：后续必须实现绑定 revision 的 `PREPARED / COMMIT_DECIDED` journal 与启动恢复；在此之前不能声称 Provider 事务具备崩溃原子性。

### 12. 验收工具曾不认识合法的聚合态

- **当时怎么做的**：`verify-proxy.ps1` 只接受单后端 `custom/deepseek/openai_configured`，把 `aggregate_upstreams=3` 且三者 false 的合法配置判失败。
- **代价是什么**：R1/R3 明知产品真实切换成功仍留下红项，发布证据含混，容易诱导无关修复。
- **根因**：产品状态机扩展后，外围验收脚本没有纳入同一个契约和回归矩阵。
- **现在的规矩**：能力变更必须同步盘点诊断、发布和验收消费者；工具不支持新合法态时要明确标待人工，不能篡改产品去迎合旧脚本。

## 四、这个代码库的地雷

1. **运行中的 GUI 会占用默认 debug 产物。** Windows 可能让 `cargo test`/链接报 os error 5。不要关闭用户正在运行的启动器；改用例如 `$env:CARGO_TARGET_DIR='E:\CSA-Worktrees\targets\csa-v018'` 的独立目录。
2. **`api_update_config` 是局部 merge。** 出口修复只能提交 `{"outbound_proxy_url": ...}`；整份旧配置覆写会冲掉 Key、Provider、模型或并发更新。
3. **`bridge_config_patch_for_api_key()` 故意不含 `outbound_proxy_url`。** 切 Key 不会恢复旧死代理；不要为“配置完整”把该字段加回 Provider patch。
4. **ALLOW 只有两个输入。** `launcher/src/laneContract.ts` 与 `allow_inputs_frozen` 是门锁；磁盘、p9、identity、沙盒和上游出口都不能成为第三把锁。
5. **WSL 运输会跨多层解析。** 禁止 `bash -lc` 塞多行、直接 argv 裸 `*`、未保护 `$()`；secret 只能走严格限长 stdin envelope，不能进入 argv、日志或 DTO。
6. **9876 配置写入需要受管身份和锁。** 写前验证 listener/source/hash/starttime，Provider 切换与出口修复共用 service/transition lock，不能另造旁路 POST。
7. **回滚只能回目标字段。** 出口失败只恢复旧 `outbound_proxy_url`；整份备份覆盖会冲掉同时发生的 Key/模型变化。备份仍要 0600 且留作人工证据。
8. **Provider 事务尚无 durable journal。** `PREPARED / COMMIT_DECIDED` 完成前，启动恢复必须把 Bridge revision 与 Windows settings 不一致视为已知欠账，不能静默猜测。
9. **PREVIEW 不是 LIVE。** `isTauri=false` 的 fixture 只能证明按钮接线；DPAPI、Bridge-only restart、真实上游和 revision 必须用 Tauri/TEST/LIVE 证据。
10. **正式发布有独立门。** 本地 debug、feature-candidate 与正式 ZIP 不可混称；ZIP、SHA-256、包内 secret scan、双入口、短路径 self-test 和并排升级只在发布任务执行。
11. **版本与当前运行进程可能不同。** GUI 锁住的旧 debug EXE 不会自动加载新 Rust；验收必须记录所用产物路径/commit，smoke 应从本轮独立目标目录直接执行。
12. **合同当前正文到 §8，五问在 `PR_CHECKLIST.md`。** 不要为了满足“§9”字样擅自改长期合同；以下五问按已冻结检查表原文执行。

## 五、验收该怎么做

### 开工前

- 记录分支、HEAD、`git status --short`；用户未跟踪文件不读取、不改、不提交，除非任务明确要求。
- 回答第六节五问，标明 ALLOW/GRADE/WORK 归属和错误前缀。
- 对照 `CURRENT_CONTRACT.md` 画受影响调用图；涉及主按钮或首屏时先写“误报会让能用系统变不能用”的反例。
- 明确现场边界：是否允许停止服务、发送计费请求、修改配置或制作正式包。没有授权就标待人工。

### 自动化门

- `cd launcher; npm run test:lanes`：现有期望值不得修改，尤其是 `allow_inputs_frozen` 和 `all_probes_red_button_still_open` 的语义。
- `cd launcher; npm run build`：TypeScript 与 Vite 同时通过。
- `cd launcher/src-tauri; cargo fmt --all -- --check`。
- 在独立 `CARGO_TARGET_DIR` 运行完整 `cargo test --jobs 1`；ignored 项必须说明为什么未跑，不能偷改为通过。
- 从仓库根运行 `pwsh -File scripts/self-test.ps1`；发布任务再跑 release gate、package policy、包内 self-test 和 secret scan。
- 运行 `git diff --check`，检查没有意外改动、生成物、明文 Key、token、用户名或本机绝对证据路径进入 diff。

### 产品路径门

- 用本轮独立 debug 产物运行 `--smoke`；required 的 ALLOW/paint/open/bridge 必须 PASS，WORK/GRADE 失败必须保持 `gating=false`。
- 记录耗时、错误码、PID/端口拓扑和产物 commit；不要只贴“exit 0”。
- UI 改动按 A–E 能力表逐项点击两套皮肤；能力入口数、供应商目录、二次确认和恢复焦点必须一致。
- 真实写操作必须记录 before/after revision、目标字段、备份、读回、验证和回滚；可能计费步骤先单独征得确认。
- 故障验收同时验证反面：探针全红时主按钮仍可打开；第一次删除不能删除；取消不能改变配置；晚到结果不能覆盖新状态。

### 交付门

- 报告按 `LIVE / PREVIEW / TEST / 待人工` 标证据，不把源码存在或 fixture 写成现场成功。
- `TASKS_PROGRESS.md` 每阶段追加“做了什么 / 证据在哪 / 下一步”三行，并指向 commit、测试和报告。
- 一个阶段一个可审查提交；P0 文档、功能代码、验收文档分开，便于回滚和追责。
- 最终复查 `git diff --stat` 与行数变化；新增复杂度必须对应明确收益，遗留项必须列出 owner/下一步。

## 六、防膨胀五问

每个 PR / 任务开头强制回答以下五问：

1. 这条改动属于 **ALLOW / GRADE / WORK** 哪一套？
2. 它替代了什么？为什么不能复用现有入口？（要给功能收益 / 复杂度成本的比较，不能只说「更好」）
3. 失败时的**错误前缀**是什么？（必须落在 `CURRENT_CONTRACT.md` §6 的表里）
4. 它是否出现在 `open_claude_science` 或**首次 paint** 的调用图里？
   - 若是 → 必须附「误报会让能用的系统变不能用」的**反例测试**，并走 `CURRENT_CONTRACT.md` §4.4 的税单
   - 若否 → 说明它挂在哪条旁路上
5. 是否新增 **daemon / 外部协议 / 首页模块**？稳定线一次最多一个，**本轮答案必须是「否」**。

**默认值规则：进不了 ALLOW 的检查，自动落 GRADE。**
