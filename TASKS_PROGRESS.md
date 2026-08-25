# CSA T7 Merge Progress

- [x] T7-1 合并切换可靠性修复与 API Key 预选确认
- [x] T7-2 给方案一/方案二增加预选确认
- [ ] T7-3 构建、切换运行实例并验收

## T7-1

做了什么：已确认工作副本为 `codex/csa-v0.1.5-model-roles`；合并前未提交的聚合功能改动已保存到可恢复 stash `codex-pre-t7-model-roles-working-tree`。

证据在哪：`git stash list` 与当前分支状态。

下一步：cherry-pick `e871469`、`99b2219`，再恢复聚合改动并按 T0-T6 新实现优先解决冲突。

## T7-1 完成

做了什么：cherry-pick 两个修复提交；冲突处保留聚合数据结构和界面，同时采用 T0-T6 的 Bridge 启动、revision/source 校验、上游预检、日志与回滚实现；新增 API Key 继续只保存不激活。

证据在哪：合并提交 `4ab4460`、`c540b26`；`restart_bridge_after_config` 只有函数末尾的成功返回，不再有静默早退。

下一步：把同一套预选确认状态机扩展到方案一/方案二。

## T7-2 完成

做了什么：方案按钮只加载并高亮草案；新增取消与确认切换；只有确认函数调用 `activate_aggregate_scheme`；API/聚合页签只切换视图；聚合激活前逐路预检三条上游。

证据在哪：`npm run build` 通过；Rust 测试 `aggregate_scheme_ui_requires_pending_confirmation`、`api_key_ui_requires_pending_confirmation` 和切换可靠性测试通过；完整 Rust 结果为 52+9 passed、0 failed、3 ignored。

下一步：运行代理回归、构建最终 debug exe、校验时间戳和 Bridge 来源，然后关闭旧实例并启动新版。

## T7-3 验收停止

做了什么：旧实例已退出，新 debug 启动器 PID `190080` 已运行；Bridge 来源与 API Key 预选确认链路完成真实验收。连续预选 `OpenCode Go`、`MiniMax`、`LongCat` 时 Bridge PID 与 revision 均不变；只点一次“确认切换”后，Bridge 仅从 PID `4045196` 变为 `4046269`，revision 仅更新一次，实际后端请求记录为 `custom / LongCat-2.0 / success`。

证据在哪：`/health.source_path` 为 `/mnt/c/Users/Admin/Documents/New project 5/csa-v0.1.5-model-roles/proxy.py`；切换前 revision `190640-1786281097675014600`，切换后 revision `190080-1786288134669273000`；截图 `C:\Users\Admin\AppData\Local\Temp\csa-t7-api-pending.png`、`C:\Users\Admin\AppData\Local\Temp\csa-t7-after-api.png`。

当前阻塞：API 切到 `LongCat` 后，聚合页的方案一被自动变成三个 `LongCat`，并显示“方案一有未应用修改”。此状态阻止预选方案二，“确认切换”也不可用，因此无法完成方案一/方案二的零重启与单次重启验收。按“任一项不通过即停止、不瞎改”的要求，T7-3 保持未完成。

下一步：单独定位 API 激活为何会改写聚合方案草稿；修复后从“API 切换完成后进入聚合页”这一现场重新执行方案预选与确认验收。

---

# CSA v0.1.5 Release R0-R7

- [x] R0 确认场地 + 建台账
- [ ] R1 发布前复盘检测（待人工：`verify-proxy.ps1` 不识别 aggregate-only 配置）
- [x] R2 最小加固：预检进程 20 秒超时
- [ ] R3 加固后回归（待人工：同 R1 的 aggregate-only 验证脚本兼容问题）
- [x] R4 整理提交
- [x] R5 打包 release 便携版
- [x] R6 发布包上检测
- [x] R7 GitHub、页面与交付报告

## R0 完成

做了什么：确认工作副本为 `csa-v0.1.5-model-roles`，当前分支为 `codex/csa-v0.1.5-model-roles`，远端为 `Dalaoyuan2020/claude-science-assistant`；建立 R0-R7 发布清单。

证据在哪：`git rev-parse --show-toplevel`、`git branch --show-current`、`git remote get-url origin`；开工时 `git status --porcelain` 为 22 项。

下一步：执行 R1 源码构建、Rust/代理回归和命令行聚合切换实测。

## R1 完成（1 项待人工）

做了什么：`npm run build` 成功（34 modules）；`cargo test` 为 52 个库测试 + 9 个集成测试通过、0 failed、3 ignored；`self-test.ps1` 为 53 translation tests passed。命令行实测 scheme-2 → scheme-1 用时 `54.8448143s`，scheme-1 → scheme-2 用时 `48.9163934s`，两次真实请求均成功。

证据在哪：两次健康结果均为 `aggregate_upstreams=3`；scheme-1 revision `193616-1786294510604921800`，scheme-2 revision `193616-1786294560557634000`；两次请求均 `responseIdPresent=true`，最近路由均记录 `custom / MiniMax-M3 / success`。`verify-proxy.ps1` 失败原因为它只检查单后端 `custom/deepseek/openai_configured`，当前合法聚合状态为三者 false、`aggregate_upstreams=3`，任务书禁止修改该脚本，标待人工。

下一步：R2 只给 `test_api_key_impl` 的 PowerShell 进程增加 20 秒外层超时，不改 45 秒请求超时、Bridge-only 或预检调用。

## R2 完成

做了什么：新增可通过 stdin 传递敏感输入的 `run_powershell_with_stdin_timeout`，仅将 `test_api_key_impl` 的执行改为外层 `20s` 硬超时；超时会终止子进程并返回“API Key 预检在 20 秒内没有响应”。

证据在哪：`launcher/src-tauri/src/lib.rs`；脚本内两处 `Invoke-RestMethod -TimeoutSec 45` 保持不变，`CSA_BRIDGE_ONLY=1` 和两条切换路径上的预检调用保持不变；`cargo build --jobs 1` 通过，耗时 `18.09s`。

下一步：执行 R3，完整重跑前端、Rust、self-test 和 verify-proxy。

## R3 完成（1 项待人工）

做了什么：加固后重跑 `npm run build`、完整 `cargo test`、`self-test.ps1`、`verify-proxy.ps1`。前三项结果与 R1 一致，没有 R2 引入的退化。

证据在哪：前端 34 modules；Rust 52 + 9 passed、0 failed、3 ignored；self-test 为 53 translation tests passed。`verify-proxy.ps1` 仍在 health 第一步因只识别单后端配置而失败，与 R1 完全相同。

下一步：R4 分类当前改动，排除本机产物和临时文件，整理 v0.1.5 发布提交。

## R4 完成

做了什么：将聚合功能、20 秒预检超时和启动器版本标识拆成三个英文提交；新增 `launcher/src-tauri/target-*/` 忽略规则，排除本机构建目录。

证据在哪：提交 `037968d feat: add aggregate subscription schemes`、`8a4067b fix: bound API key preflight execution`、`f5edf38 feat: display the v0.1.5 launcher version`。

下一步：R5 使用现成脚本制作 `release-v0.1.5-publish-20260810`。六份 `CSA_T8*20260809.md` 草案与本轮发布任务冲突且归属无法确认，保留为未跟踪文件并标待人工，不进入提交。

## R5 完成

做了什么：以 `release` profile、单 Cargo job 构建 v0.1.5，并用 `package-launcher-portable.ps1` 生成完整便携包。首次封装发现当前工作副本缺少被 `.gitignore` 排除的 Linux 运行时；随后从 v0.1.4 稳定工作副本补入 Claude Science 0.1.25，并在封装前校验其 SHA-256 与当前仓库 manifest 完全一致。

证据在哪里：运行时 `--version` 为 `claude-science 0.1.25 (release, public)`，SHA-256 为 `C663367BBC7EC54E7D1E5A9102594A9E70804ED5070F5D7CD1117E665E3C376C`；产物位于 `dist/release-v0.1.5-publish-20260810/claude-science-assistant-v0.1.5-publish-20260810-release-portable.zip`，并已生成同名 `.sha256`。

下一步：R6 从发布包本身核验 EXE 时间戳、关键前端文案、包内 self-test 和 ZIP SHA-256。

## R6 完成

做了什么：为便携包补充同构建生成的前端 JS 校验副本，以及 self-test 依赖的三份只读审计源文件；从最终 ZIP 解压到短路径后执行包内自检，并独立复算 ZIP 哈希。

证据在哪里：最新打包源码 `scripts/package-launcher-portable.ps1` 时间为 `2026-08-10T01:28:22+08:00`，包内 EXE 时间为 `2026-08-10T01:30:24+08:00`；包内 `index-Bakxun1r.js` 对“确认切换”“方案一”“方案二”各命中 1 次；包内 self-test 为 `53 translation tests passed`、`self-test passed`；ZIP 大小 `90,677,168` 字节，SHA-256 为 `82D8AA53BEAEA4D09E1E3AA41B318AD86675F3976EFA6688CD87CF8CFDF9814A`，与 `.sha256` 一致。

补充说明：直接在超长开发路径运行包内 self-test 会触发 Windows 传统路径长度限制；从同一最终 ZIP 解压到短路径后完整通过。这不影响启动器运行，但自检建议在短目录执行。

下一步：R7 执行凭据扫描、更新 README/Release 说明与交付报告，再推送分支、标签和正式 Release。

## R7 完成

做了什么：完成仓库与最终包凭据扫描；更新 README、v0.1.5 Release 说明和发布报告；推送 `codex/csa-v0.1.5-model-roles` 分支，创建并推送 `v0.1.5` 标签，发布非草稿、非预发布的 GitHub Release。

证据在哪里：仓库 120 个文本文件与包内 64 个文本文件对 Telegram/OpenAI/Anthropic/GitHub/Bearer 凭据模式均为 0 命中；Release 为 `https://github.com/Dalaoyuan2020/claude-science-assistant/releases/tag/v0.1.5`；GitHub 反查 ZIP 为 `90,677,168` 字节，SHA 文件为 `137` 字节。

待人工清单：`verify-proxy.ps1` 增加 aggregate-only 识别；目标电脑复验视觉订阅图片请求和旧版并排升级；确认六份本地 `CSA_T8*20260809.md` 草案应保留、合并还是删除。这些事项未混入本次 tag 和 Release。

下一步：人工按 `docs/reports/CSA_v015_release_report_20260810.md` 第一段执行两项 10 分钟内复验。

---

# CSA v0.1.6 结构减负（MASTER 单文件任务）

## S0 完成

做了什么：以 `03dc6c2` 为基线完成只读测绘；核对 20 个 Tauri command、首屏/打开/30 秒刷新三张调用图、二元 ALLOW 判据和全部现有探针链路，未改产品代码。

证据在哪：`docs/reports/CSA_v016_S0_survey_20260824.md`；其中记录 `lib.rs=7521` 行、`main.rs=6` 行、`App.tsx=2001` 行，以及 Bridge `outbound_proxy_url` 在现有启动器/体检链路中 0 命中的结构缺口。

下一步：进入 S0.5，在 `main.rs` 增加进程内 `--smoke`，并以 debug 构建验证 allow/paint/open/bridge/egress/grade，不启动第二个 GUI 或 Bridge。

## S0.5 完成

做了什么：新增 debug `--smoke [--only ...]` 进程内分流；ALLOW 用 550ms 发行版枚举 + 单次 2.2 秒 WSL 快探针，端口占用与 PID 可见性分离，Windows Bridge 使用 `Absent/Present/PortConflict/Unknown` 四态；open dry-run 与 GUI 共用 7 秒内部硬期限和客体进程组回收；Bridge `/health`+`/v1/models` 同时对拍当前包四文件 bundle、9876 listener PID、受管 current 指针和 required path-secret。薄壳 `scripts/csa-smoke.ps1` 先增量 debug build，杜绝陈旧 EXE 假绿。smoke 不进入 Tauri GUI、不启动/停止服务、不弹浏览器，ALLOW/paint 共用一次快照。

证据在哪：`launcher/src-tauri/src/smoke.rs`、`launcher/src-tauri/src/main.rs`、`launcher/src-tauri/src/runtime_lifecycle.rs`、`scripts/csa-smoke.ps1`；Rust 81 passed、0 failed、4 ignored；现场运行后仍为 Claude PID 443、Bridge PID 18733、2222 保持监听，未留下第二个启动器进程。完整 smoke 输出如下。

下一步：进入 S1，将已验证的 ALLOW 快路径正式暴露为 `get_allow_status`，拆出 `AllowStatus / GradeStatus / WorkReport`，并以 `allow_inputs_frozen` 同时冻结 canOpen 输入与主按钮文案输入。

```text
PASS allow 475ms inputs=claudeRunning,windowsBridgePid distro=Ubuntu-24.04 linuxUser=lyuwinnie claudeRunning=true claudePid=443 windowsBridgePid=none windowsBridgeProbe=checked runtimePresent=true listenerProbeOk=true listenerPresent=true daemonState=managed_ready pid8765=443 pid8766=443 controlSocket=true canOpen=true canStart=false
PASS paint 475ms budget=3000ms
PASS open 789ms login.url_ready loopback=true port=8765 nonce=present
PASS bridge 502ms health=200 models=200 identity=current modelCount=5
FAIL egress 0ms work.bridge_egress.not_implemented
PASS grade 5524ms state=running warnings=0 gating=false
SUMMARY required_pass=4 required_fail=0 non_gating_fail=1 elapsed_ms=7291 exit=0
```

## S1 完成

做了什么：正式拆出 `AllowStatus / GradeStatus / WorkReport` 与 `get_allow_status / get_grade_status` command；GUI 与 smoke 共用同一 ALLOW/Grade 实现。主按钮的可打开判据冻结为 `claudeRunning && !windowsBridgePid`，文案只从 `AllowStatus` 产生且固定为四条；`restartBlocked`、聚合 `state`、出口与深检不再参与主按钮。已知启动、登录、Bridge、Grade、Work 与 WSL 运输故障使用分层前缀和不同后缀。缺少 WSL 或发行版时 ALLOW 返回可渲染状态，不再把首屏本身变成错误。

证据在哪：`launcher/src-tauri/src/lib.rs` 的 `allow_inputs_frozen` 同时执行断言 A（canOpen 输入集合严格等于二元冻结集合）与断言 B（主按钮文案输入只能是 ALLOW 字段），并验证产品实际调用纯函数；三车道序列化反向测试阻止 DTO 串线。Rust 为 86 passed、0 failed、4 ignored；`npm run build` 通过（34 modules）。完整 smoke 输出如下。

下一步：进入 S2，把 `initialize_runtime` 移到首次 ALLOW paint 之后的后台；30 秒刷新只取 Grade 并只合并 Grade，生命周期动作完成后单独刷新 ALLOW；补首屏/周期/按钮源码边界测试。

```text
PASS allow 533ms inputs=claudeRunning,windowsBridgePid wslInstalled=true distro=Ubuntu-24.04 linuxUser=lyuwinnie claudeRunning=true claudePid=443 windowsBridgePid=none windowsBridgeProbe=checked runtimePresent=true listenerProbeOk=true listenerPresent=true daemonState=managed_ready pid8765=443 pid8766=443 controlSocket=true canOpen=true canStart=false
PASS paint 533ms budget=3000ms
PASS open 837ms login.url_ready loopback=true port=8765 nonce=present
PASS bridge 492ms health=200 models=200 identity=current modelCount=5
FAIL egress 0ms work.bridge_egress.not_implemented
PASS grade 4497ms state=running warnings=0 gating=false
SUMMARY required_pass=4 required_fail=0 non_gating_fail=1 elapsed_ms=6361 exit=0
```

## S2 完成

做了什么：把首屏拆为 `refreshAllow / refreshGrade / initializeRuntimeInBackground` 三条独立回路。首次只等待 ALLOW，提交按钮状态后跨过一个 `requestAnimationFrame` 才在后台幂等初始化；初始化不再写全局 busy、不再全量 `setStatus`，完成后只重读 ALLOW。30 秒定时器只调用 `get_grade_status`；Grade 使用 functional merge，只更新仪表字段并保留当前 WORK 深检结果。启动/停止/重启动作忽略旧 `SystemStatus` 返回，随后分别刷新 ALLOW 与 Grade。Claude PID、发行版、旧 Windows Bridge 等身份展示统一读取 `allowStatus`。

证据在哪：`launcher/src/App.tsx`；Rust 源码边界测试 `first_paint_allow_only_initialize_background`、`periodic_refresh_is_grade_only_and_preserves_allow_and_work`、`primary_button_is_not_blocked_by_grade_or_work` 均通过。Rust 为 89 passed、0 failed、4 ignored；`npm run build` 通过（34 modules）。现场 smoke 的 paint 为 535ms、open 为 724ms，均远低于 3s/8s 硬预算。完整输出如下。

下一步：进入 S3，把主按钮 selector 与 ALLOW/Grade/Work reducer 做成可执行纯模块，构造全部探针红灯的真实 fixture，证明按钮文案、disabled、PID 与端口完全不变；同时给非阻断探针补 UI 硬期限与熔断。

```text
PASS allow 535ms inputs=claudeRunning,windowsBridgePid wslInstalled=true distro=Ubuntu-24.04 linuxUser=lyuwinnie claudeRunning=true claudePid=443 windowsBridgePid=none windowsBridgeProbe=checked runtimePresent=true listenerProbeOk=true listenerPresent=true daemonState=managed_ready pid8765=443 pid8766=443 controlSocket=true canOpen=true canStart=false
PASS paint 535ms budget=3000ms
PASS open 724ms login.url_ready loopback=true port=8765 nonce=present
PASS bridge 381ms health=200 models=200 identity=current modelCount=5
FAIL egress 0ms work.bridge_egress.not_implemented
PASS grade 3947ms state=running warnings=0 gating=false
SUMMARY required_pass=4 required_fail=0 non_gating_fail=1 elapsed_ms=5589 exit=0
```

## S3 完成

做了什么：新增生产级 `laneContract.ts`，由 App 实际使用三车道 reducer 与纯 ALLOW 主按钮 selector；主按钮只读取 `AllowStatus + allowLoaded + allowActionBusy`，与设置 busy、Grade、Work 完全分离。Grade、沙盒深检与运行时更新统一使用硬期限、熔断、机器码分类和迟到结果丢弃；timeout/未测到显示灰色，确定故障显示红色并附“仍可打开 Claude Science”。运行时更新后端共享 40 秒总截止时间（小于前端 45 秒），API Key 自动映射的 PowerShell 进程补为 20 秒硬期限。

证据在哪：`launcher/tests/all-probes-red.test.ts` 的 `all_probes_red_button_still_open` 构造出口 502、DrvFS D/p9、磁盘告警、代理冲突、identity 不匹配、canary/版本超时等全红组合，断言按钮仍为“打开 Claude Science”、`disabled=false` 且 ALLOW PID/8765/8766 不变；另有后端提前 timeout、语义 timeout 熔断、并发 supersede 与晚结果反例。Node 6 passed、0 failed；Rust 91 passed、0 failed、4 ignored；`npm run build`、self-test（54 translation tests + 35 passed/3 skipped）和独立最终复核均通过。完整 smoke 输出如下。

下一步：进入 S4，在 WORK 车道新增 Bridge → 上游模型 API 的真实出口探针；复用 smoke 同一实现，现场识别 10808 死代理导致的 502，并只生成需用户确认的修复 Prompt，不自动改系统代理或 Key。

```text
PASS allow 429ms inputs=claudeRunning,windowsBridgePid wslInstalled=true distro=Ubuntu-24.04 linuxUser=lyuwinnie claudeRunning=true claudePid=443 windowsBridgePid=none windowsBridgeProbe=checked runtimePresent=true listenerProbeOk=true listenerPresent=true daemonState=managed_ready pid8765=443 pid8766=443 controlSocket=true canOpen=true canStart=false
PASS paint 429ms budget=3000ms
PASS open 723ms login.url_ready loopback=true port=8765 nonce=present
PASS bridge 329ms health=200 models=200 identity=current modelCount=5
FAIL egress 0ms work.bridge_egress.not_implemented
PASS grade 4008ms state=running warnings=0 gating=false
SUMMARY required_pass=4 required_fail=0 non_gating_fail=1 elapsed_ms=5491 exit=0
```

## S4 完成

做了什么：新增唯一权威入口 `run_bridge_egress_probe(confirm_billable)`，由 Tauri `run_bridge_egress_check` 与 smoke 的 `egress` 项共同调用。探针在 Bridge 所在 WSL 网络命名空间内按 `/health → outbound proxy TCP → /v1/models → 单次 max_tokens=1 /v1/messages → 不经 proxy 的直连对照` 分层执行；64 秒 host watchdog 与 62 秒 guest timeout 双重回收。当前 10808 在第二层 TCP refused 后立即短路，后三层 skipped，绝不为了复现既知 502 继续计费。required path secret、Bridge current path/hash/listener PID/cmdline、model allowlist、URL origin-only 与输出 control-char 合同均在 WSL 内校验；不调用 `/api/config`，不把 Key、token、请求头、响应正文或 URL path/query/fragment送回 DTO。guest timeout 124/137 映射为语义 timeout。

前端在既有“沙盒 / API 出口”状态项内增加第二个“能力体检”动作，没有新增首页模块。第一次点击只打开说明；只有第二次点击“同意并开始体检（1 次真实请求）”才以 `confirmBillable: true` 调 command。它使用独立 WORK busy/ref/circuit 与 75 秒 deadline，不写 ALLOW、PID、全局启动 busy 或主按钮。结构化红色报告仍保留五层证据、是否计费与 `buildBridgeEgressRepairPrompt()`；Prompt 只建议等待用户批准后清空 `outbound_proxy_url` 或改为 WSL 内真实监听地址，禁止自动改系统代理/VPN/DNS/hosts/证书/443、关闭 WSL或触碰 2222。

证据在哪：`launcher/src-tauri/src/bridge_egress.rs`、`launcher/src-tauri/src/lib.rs`、`launcher/src-tauri/src/smoke.rs`、`launcher/src/bridgeEgress.ts`、`launcher/src/App.tsx`、`launcher/tests/bridge-egress.test.ts`。Rust 95 passed、0 failed、4 ignored，integration 9 passed；Node 9 passed；`npm run build` 通过（36 modules）；pwsh 总 self-test 为 54 translation tests、package policy、35 passed/3 skipped。独立后端与前端最终审查均无 blocker。浏览器实际点击证据：首次点击只出现计费确认，取消后 dialog=0；二次确认后五层结果完整，主按钮在弹窗前/确认中/结果后均 enabled；Escape 可关闭并把焦点还给“能力体检”；800px 视口为两列 `334.5px 334.5px`、最后一层通栏且无水平溢出。

现场证据：最终 smoke 精确查出 `http://127.0.0.1:10808` 对应的 `work.bridge_egress.proxy_dead`，`billable=false` 且 `gating=false`；required checks 仍全绿、exit=0。smoke 前后 Claude PID 443、Bridge PID 18733 未变，8765/8766/9876 与无关 2222 均保持监听。完整输出如下。

下一步：进入 S5，落长期合同与 PR 防膨胀五问，并逐项点击现有 UI/command，记录每项入口、响应和日志证据；任一无法现场安全点击的破坏性/计费能力明确标为待人工，不用源码存在冒充已验收。

```text
PASS allow 426ms inputs=claudeRunning,windowsBridgePid wslInstalled=true distro=Ubuntu-24.04 linuxUser=lyuwinnie claudeRunning=true claudePid=443 windowsBridgePid=none windowsBridgeProbe=checked runtimePresent=true listenerProbeOk=true listenerPresent=true daemonState=managed_ready pid8765=443 pid8766=443 controlSocket=true canOpen=true canStart=false
PASS paint 426ms budget=3000ms
PASS open 724ms login.url_ready loopback=true port=8765 nonce=present
PASS bridge 380ms health=200 models=200 identity=current modelCount=5
FAIL egress 381ms work.bridge_egress.proxy_dead health=passed proxy=failed models=skipped request=skipped direct=skipped billable=false gating=false
PASS grade 3311ms state=running warnings=0 gating=false
SUMMARY required_pass=4 required_fail=0 non_gating_fail=1 elapsed_ms=5224 exit=0
```

## S5 进行中（合同与安全入口已落地；高风险验收待人工）

做了什么：将任务书 §1–§8 原文冻结为长期合同，并落下防膨胀五问与“非 ALLOW 默认进 GRADE”规则；按 A–E 能力表逐项核对入口。真实 Tauri 中完成刷新、深检、计费说明取消、存储 Prompt、运行时索引与升级/回退 Prompt、供应商/聚合预选取消、打开 Claude Science 等只读点击；浏览器 preview 完成 dummy Key、模型映射和聚合事务接线点击，并明确只算 fixture。验收同时修复全部 preview 成功分支残留旧错误、隐藏 DrvFS 副作用的“重启”文案、三处 `bash -lc` 运输债，以及配置面板 `--noproxy "*"` 在 DrvFS 中被展开导致的 8 秒超时；dashboard origin 进一步固定为 `127.0.0.1:9876`，即使配置中的 host/port 被篡改也不能把 path secret 带离 loopback。

证据在哪：`docs/architecture/CURRENT_CONTRACT.md` 与 MASTER §1–§8 逐行一致；`docs/architecture/PR_CHECKLIST.md` 含五问；34 项能力入口、响应、证据口径和待人工项记录在 `docs/reports/CSA_v016_S5_capability_audit_20260825.md`。Node 11 passed、0 failed；前端 build 35 modules；Rust 库 100 passed、0 failed、4 ignored，独立目标目录集成测试 9 passed、0 failed；总 self-test 为 54 translation tests、package policy、35 passed/3 skipped。最终 smoke 使用独立目标目录的新产物，完整输出如下。

待人工：维护窗口中的启动/停止/修复并重启、旧 Windows Bridge fixture；需要用户明确费用/凭据授权的真实 Key、模型读取、真实接入切换、聚合提交和 egress 第二次确认；发布阶段的 ZIP、SHA-256、包内 secret scan、双入口与并排升级。根据 MASTER“任一条点不动 = S5 未完成”，本阶段不虚报全绿，本轮也不制作正式 Release ZIP。

下一步：用户提供维护窗口及费用/真实凭据操作的明确批准后，完成剩余现场点击；全部能力全绿后再进入正式发布验收。

```text
PASS allow 322ms inputs=claudeRunning,windowsBridgePid wslInstalled=true distro=Ubuntu-24.04 linuxUser=lyuwinnie claudeRunning=true claudePid=443 windowsBridgePid=none windowsBridgeProbe=checked runtimePresent=true listenerProbeOk=true listenerPresent=true daemonState=managed_ready pid8765=443 pid8766=443 controlSocket=true canOpen=true canStart=false
PASS paint 322ms budget=3000ms
PASS open 776ms login.url_ready loopback=true port=8765 nonce=present
PASS bridge 416ms health=200 models=200 identity=current modelCount=5
FAIL egress 379ms work.bridge_egress.proxy_dead health=passed proxy=failed models=skipped request=skipped direct=skipped billable=false gating=false
PASS grade 3708ms state=running warnings=0 gating=false
SUMMARY required_pass=4 required_fail=0 non_gating_fail=1 elapsed_ms=5603 exit=0
```

## V0.1.7 T0 完成

做了什么：在 `codex/csa-v0.1.7` 上完成只读出口基线；确认 Bridge 与 Windows 系统代理均为 `http://127.0.0.1:12334`，并复测直连、当前代理与旧死口 10808 三条路线。

证据在哪：`docs/reports/CSA_v017_T0_egress_baseline.md` 保存 `/health`、九次无凭据 curl 原始输出、`lib.rs` 感知计数与完整 smoke；当前代理三组目标均达认证/HTTP 层，10808 三组均拒绝，smoke 为 `required_fail=0`、`work.bridge_egress.ok`、`gating=false`。

下一步：进入 T1，在既有 WORK 探针内发现并分层验证候选，输出排序与推荐；不进入 ALLOW、首屏或周期刷新。
