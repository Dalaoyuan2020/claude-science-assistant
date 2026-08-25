# CSA V0.1.6 S5 能力核对（2026-08-25）

> V0.1.7 功能候选补充：本文前半部分保留 V0.1.6 当时的现场快照。后续经用户明确授权，已完成受管后端的真实 Key 切换、恢复和最终 egress smoke；以文末“V0.1.7 补充验收”为最新结论。界面仍显示 `0.1.6`，因为本轮交付的是 v0.1.7 feature-candidate 源码，不是正式 V0.1.7 Release 包。

## 结论

长期合同和反膨胀门已经落仓；只读、无计费的真实 Tauri 入口已经逐项点击，浏览器预览中的事务界面也已逐项点击。S5 尚不能写成“能力清单全绿”：会停止/重启服务、切换真实 Key/聚合路由、发送可能计费请求、制作正式 Release 或模拟旧 Bridge 的项目仍明确标为 `待人工`。这符合 MASTER 的诚实验收规则，不能用源码存在或 preview fixture 冒充现场成功。

原 S5 快照没有关闭 WSL、没有关闭启动器、没有触碰 2222，没有修改系统代理/VPN/DNS/hosts/证书/443，没有制作正式 Release ZIP，也没有发送新的模型请求。V0.1.7 补充验收继续遵守这些系统边界，但按任务授权发送了三条 `max_tokens=1` 请求：OpenRouter 切换验证、DeepSeek 恢复验证、最终 smoke。

## 证据口径

| 标记 | 含义 |
|---|---|
| `LIVE` | 在当前 debug Tauri 启动器中真实点击并观察响应 |
| `PREVIEW` | 在浏览器预览中真实点击 UI，但后端结果是明确的 fixture，只证明界面接线 |
| `TEST` | 负向不变量或无独立 UI 的能力，由自动化测试/现场日志证明 |
| `待人工` | 当前点击会改变用户服务、凭据、计费或发布状态，未冒险代点 |

现场基线：Ubuntu-24.04；Bridge PID 18733/9876；Claude Science PID 443/8765+8766；2222 同时保持 IPv4/IPv6 监听。真实 UI 的“当前接入”为 DeepSeek；报告只记录供应商名称和“已加密保存”状态，不读取或回显任何 Key。

## 真实 UI 点击日志

| 入口 | 点击结果 |
|---|---|
| 刷新状态 | `Claude Science 已准备好`；Bridge PID 18733；Claude PID 443 |
| 深度检测 | 完成；`analysis/SOCKS5H`，`SOCKS 握手与实链路通过 HTTP 200` |
| 能力体检 | 第一次点击只出现 `max_tokens=1` 计费说明；点击取消，未发送模型请求 |
| 存储建议 | 弹出本机化 Prompt；明确“不停止 WSL、不移动 VHDX、不执行 unregister” |
| 检查更新 | 读取到 official stable/latest 0.1.27；仍锁定已验证 0.1.25，不自动覆盖 |
| 升级 Prompt / 回退 Prompt | 两个隔离验证 Prompt 均正常弹出；未执行替换 |
| API Key 列表 | 预选 OpenRouter 后显示待切换；点击取消；当前接入仍为 DeepSeek，Bridge PID 未变 |
| 添加供应商 | 官方、聚合、中转、自定义四组目录均可见；未输入或保存 Key |
| 聚合模式 | 方案一、方案二和三条路由均可见；预选方案二后取消，未提交 |
| 打开 Claude Science | 点击后启动器仍为 ready；前端无 nonce；同一路径 smoke 证明 loopback=8765、nonce 只在 Rust |
| 配置面板 | 首次点击命中 `bridge.dashboard_url_failed`；修复后的同路径 helper 实机返回 dashboard HTTP 200（约 32 ms）。当前已运行的旧 debug GUI 需下次重启加载新 Rust 代码 |
| 修复并重启 / 停止 | 文案和入口可见；未点击，避免在未进入维护窗口时改变服务 |

浏览器预览的补充点击日志：供应商四组、dummy Key 测试、`/models` fixture、三层模型手动映射、保存到列表、预选/确认切换、聚合方案一/二和整张路由表一次提交全部有响应。它们只证明 UI wiring；`isTauri=false` 分支不会被写成真实 DPAPI、Bridge 重启或真实上游请求证据。

## A. 点火（ALLOW）

| # | 能力 | UI 入口 | 结果 | 证据 |
|---|---|---|---|---|
| A1 | 检测 WSL / 发行版 / 受管二进制 | 刷新状态、环境卡 | `LIVE` | WSL2 Ubuntu-24.04、运行时已准备；smoke `PASS allow` |
| A2 | 启动、停止、重启 Bridge + Claude Science | 主按钮、页脚停止、修复并重启 | `待人工` | command/脚本和生命周期测试存在；本轮未改变服务 |
| A3 | loopback 8765 登录页；nonce 只在 Rust | 打开 Claude Science | `LIVE+TEST` | 真实点击无前端 nonce；smoke `loopback=true port=8765 nonce=present` |
| A4 | 至多一个受管 Bridge、一个 daemon | 生命周期入口，无独立按钮 | `TEST` | 跨进程锁、listener owner、20-cycle 与现场单 PID |
| A5 | versions/current 受管路径；解压目录仅安装源 | 状态/生命周期，无独立按钮 | `TEST` | runtime layout 与 downgrade guard 测试 |
| A6 | `/health` identity 证明当前包 | 刷新状态、Bridge 卡 | `LIVE+TEST` | `PASS bridge ... identity=current` |
| A7 | 旧 Windows Bridge 阻断并显式停止 | 仅旧 Bridge 存在时出现 | `待人工` | 当前无旧实例，不能诚实声称点过；selector/后端测试存在 |
| A8 | 不自动 shutdown/terminate WSL | 无独立 UI | `TEST` | 停止脚本否定断言；2222 在所有 smoke 前后均保持监听 |
| A9 | 锁定 0.1.25；拒绝隐式降级 | Runtime Update | `LIVE+TEST` | 真实读取 0.1.27 但未覆盖 0.1.25；downgrade fixture 通过 |

## B. 接入（事务化）

| # | 能力 | UI 入口 | 结果 | 证据 |
|---|---|---|---|---|
| B1 | 官方 / 聚合 / 中转 / 自定义目录 | 添加供应商 | `LIVE+PREVIEW` | 四组均实际展开 |
| B2 | DPAPI 保存 Key；不回显明文 | 添加供应商、Key 列表 | `待人工+TEST` | 当前真实列表显示“已加密保存”；DPAPI roundtrip/DTO 脱敏测试通过；未新增 Key |
| B3 | 测连通、读 `/models`、三层建议、手调映射 | 供应商表单 | `PREVIEW+待人工` | dummy fixture 全流程已点；真实请求可能使用用户额度，未代点 |
| B4 | 预选 + 确认；浏览不重启 | Key 列表、取消、确认 | `LIVE/待人工` | 真实预选+取消后 PID 18733 不变；真实确认未点 |
| B5 | 单 API；聚合方案一/二 | API/聚合页签 | `LIVE+PREVIEW` | 两页签、两方案均可见并可预选 |
| B6 | 聚合为整张路由表一次提交 | 保存并应用整套方案 | `PREVIEW+待人工` | preview 一次提交成功；真实提交未点 |
| B7 | 切换需 Bridge/identity/revision 外证据；失败回滚 | 确认切换 | `TEST+待人工` | transaction/rollback/revision 测试存在；未改变真实接入 |
| B8 | 预检硬超时 | 测试/映射/确认，无独立入口 | `TEST` | 20 秒外层 deadline 与机器码测试 |

## C. 仪表（GRADE）

| # | 能力 | UI 入口 | 结果 | 证据 |
|---|---|---|---|---|
| C1 | 六项状态可见 | 环境状态展开/收起 | `LIVE` | WSL、运行时、Bridge、Claude、存储、当前 Key 均可见；另含出口卡 |
| C2 | identity / 双端口 / 未验证监听者区分 | Bridge、Claude、诊断 | `LIVE+TEST` | 正常态 PID 与双端口现场；异常 fixture 测试 |
| C3 | 3/3 HTTP/SOCKS 拓扑 | 沙盒/API 出口卡 | `LIVE` | `3/3 ... expected` |
| C4 | 代理冲突/可达；只清 daemon 环境 | 出口卡、诊断 | `LIVE+TEST` | 当前 sandbox/daemon 代理可达；冲突/子进程 env 测试 |
| C5 | 磁盘/VHDX/只读根告警 | WSL 存储、存储建议 | `LIVE+TEST` | 真实容量与只读 Prompt；故障 fixture 测试 |
| C6 | DrvFS 持久写授权只读检测 | 状态/诊断，无独立按钮 | `TEST` | grant inspection 与 broad/narrow 测试 |

## D. 车间（WORK）

| # | 能力 | UI 入口 | 结果 | 证据 |
|---|---|---|---|---|
| D1 | analysis socket + SOCKS5H + PyPI HEAD | 深度检测 | `LIVE` | HTTP 200；无模型请求、无计费 |
| D2 | p9 瞬态不进 15 分钟缓存；恢复即失效 | 深检结果，无独立按钮 | `TEST` | transient cache/contract tests |
| D3 | 0600 备份、DrvFS RW→RO、ext4 RW 不变 | 修复并重启 | `待人工+TEST` | 文案已恢复为显式副作用；授权/备份测试；真实重启未点 |
| D4 | 存储只读体检 + Prompt，不自动迁移 | 存储建议 | `LIVE` | Prompt 明确不 move/unregister |
| D5 | latest/stable + 隔离升级/回退 Prompt | Runtime Update | `LIVE` | 真实索引 0.1.27；两种 Prompt 均弹出 |
| D6 | MCP 不全预热、Git 按需、Python/R/BYOC 保留 | 启动路径，无独立按钮 | `TEST` | boot-mode/首装路径源码与测试 |
| D7 | `work.bridge_egress` + 修复 Prompt | 能力体检 | `LIVE+TEST/待人工` | 第一层 consent 实点后取消；`confirm=0` 现场命中 proxy_dead；计费确认未点 |

## E. 交付与安全

| # | 能力 | 入口 | 结果 | 证据 |
|---|---|---|---|---|
| E1 | 便携 ZIP + SHA-256，不只换 EXE | package 脚本 | `TEST/待发布` | 打包策略测试；MASTER 本轮默认禁止正式 Release ZIP |
| E2 | 并排升级并保留 APPDATA/DPAPI/Claude 数据 | 新目录接管 | `TEST+待人工` | current pointer/APPDATA/identity 测试；未接管真实安装 |
| E3 | BAT/PowerShell 双入口 | 包根入口 | `TEST/待发布` | package policy 保留双入口；未制作本轮正式包 |
| E4 | Token/配置/对话不进 git/发布包 | secret scan，无 UI | `TEST/待发布` | Git/Prompt 脱敏测试；正式包扫描要在发布阶段执行 |

## 验收中发现并修复

1. 浏览器预览的保存、测试、自动映射、激活、删除 API Key，以及保存/确认聚合事务成功后可能残留旧错误，造成“已生效”和“未应用”同时出现。所有 preview 成功分支现会清空旧错误；`access-preview.test.ts` 先把每个 preview 分支截断在真实 Tauri 分支之前再逐项断言，防止跨分支假绿。
2. 页脚“重启”实际上会执行持久 DrvFS 授权修复，旧文案隐藏副作用。入口改回“修复并重启”，并用 title 明说备份、收窄授权及受管进程重启。
3. 合同 §8 落盘后反查出三处旧 `bash -lc` 运输债：stop 多行脚本、PowerShell `-Open`、验收证据采集。现分别改为 stdin→`bash -s` 或独立 argv 直接执行，并补“外层会展开”回归；生产代码不再含 `bash -lc`。
4. 真实“配置面板”首次点击命中 `bridge.dashboard_url_failed`（8 秒 WSL 超时）。根因不是 Bridge 或 WSL 不通，而是经 `wsl.exe` 发送的 `--noproxy "*"` 被 WSL 在 DrvFS 当前目录展开，curl 把工作区文件名当成 URL。现改为 Windows `curl.exe --noproxy 127.0.0.1` 验证浏览器实际 loopback；optional-auth 不再启动 WSL，required-auth 才经 stdin 读取 auth mode/token 两个必要字段。Rust 直接打开 dashboard，URL/path token 不再返回前端；dashboard origin 无条件固定为 `http://127.0.0.1:9876`，恶意 `proxy_host`/`proxy_port` 也不能把 path secret 带离 loopback；`lib.rs` 直接 argv 的 no-proxy glob 已被回归测试禁止。修复后实机 dashboard HTTP 200，约 32 ms。

## 自动化与最终 smoke

- 前端：`npm run test:lanes` 为 11 passed、0 failed；`npm run build` 成功（35 modules）。
- Rust：`cargo fmt --all -- --check` 通过；独立 Cargo 目标目录的完整 `cargo test --jobs 1` 为库测试 100 passed、0 failed、4 ignored，集成测试 9 passed、0 failed。4 个 ignored 项分别需要真实用户额度、实时登录 URL 或官方网络索引，本轮没有偷跑。
- 总 self-test：54 translation tests、package policy、35 passed/3 skipped，最终 `self-test passed`。
- 最终 smoke 从独立目标目录的新源码产物直接执行，未使用被 GUI 锁住的旧 debug EXE；当前 GUI、Bridge PID 18733、Claude PID 443 与 2222 均未停止。

```text
PASS allow 322ms inputs=claudeRunning,windowsBridgePid wslInstalled=true distro=Ubuntu-24.04 linuxUser=lyuwinnie claudeRunning=true claudePid=443 windowsBridgePid=none windowsBridgeProbe=checked runtimePresent=true listenerProbeOk=true listenerPresent=true daemonState=managed_ready pid8765=443 pid8766=443 controlSocket=true canOpen=true canStart=false
PASS paint 322ms budget=3000ms
PASS open 776ms login.url_ready loopback=true port=8765 nonce=present
PASS bridge 416ms health=200 models=200 identity=current modelCount=5
FAIL egress 379ms work.bridge_egress.proxy_dead health=passed proxy=failed models=skipped request=skipped direct=skipped billable=false gating=false
PASS grade 3708ms state=running warnings=0 gating=false
SUMMARY required_pass=4 required_fail=0 non_gating_fail=1 elapsed_ms=5603 exit=0
```

## 现场 `proxy_dead` 证据

本次使用权威探针的 `confirm_billable=false` 路径，只读取得结构化报告；没有调用 `/v1/messages`：

```json
{"operation":"bridge_egress","ok":false,"code":"work.bridge_egress.proxy_dead","conclusion":"Bridge 配置的 outbound proxy 在 WSL 内未监听或不可达；已短路，未发送真实模型请求。","billableRequestSent":false,"outboundProxyConfigured":true,"health":{"state":"passed","code":"work.bridge_egress.health_ok","durationMs":6,"httpStatus":200},"proxy":{"state":"failed","code":"work.bridge_egress.proxy_dead","durationMs":0,"detail":"Configured proxy did not accept a WSL TCP connection within 1.5 seconds."},"models":{"state":"skipped","code":"work.bridge_egress.models_skipped","durationMs":0},"request":{"state":"skipped","code":"work.bridge_egress.request_skipped","durationMs":0},"direct":{"state":"skipped","code":"work.bridge_egress.direct_skipped","durationMs":0},"suggestedAction":"将 outbound_proxy_url 置空，或替换为 WSL 内实际监听且可达的代理地址。","warnings":[],"outboundProxyUrl":"http://127.0.0.1:10808"}
```

同一报告经 `buildBridgeEgressRepairPrompt()` 生成完整修复 Prompt；Prompt 的五层证据、脱敏、审批边界和验证命令由 `launcher/tests/bridge-egress.test.ts` 冻结。完整原文随 S5 最终交付回答提供。

## 待人工 / 发布阶段

- 维护窗口：真实点击停止、启动、修复并重启，并验证 2222 与无关服务不受影响。
- 旧 Windows Bridge fixture：验证显式停止入口，不能在当前无实例时假装已点。
- 明确费用授权后：真实 Key 测试/模型列表、Key/聚合确认切换，以及能力体检第二层。
- 发布阶段：完整 ZIP、SHA-256、包内 secret scan、BAT/PowerShell 双入口和并排升级。

因此，本报告把 S5 标为“合同/入口核对已落地，破坏性、计费和正式发布验收待人工”，而不是虚假的全绿。

## V0.1.7 补充验收（最新）

### 现场边界与最终状态

- 没有关闭当前启动器 GUI；没有停止 Claude Science；没有关闭整个 WSL；没有触碰 2222。
- 最终活动接入恢复为 DeepSeek：`activeApiKeyId=key-…-135948`（报告只保留脱敏后缀），`force_model=deepseek-v4-pro`。
- 最终 Bridge PID `157675` 监听 9876；Claude Science PID `108819` 同时监听 8765/8766；`outbound_proxy_url=http://127.0.0.1:12334`。
- 最终 Bridge revision 为 `166128-1787631265733624000`。

### B4 / B7 真实事务补证

UI 层仍按“预选 → 二次确认”工作；此前真实 UI 已完成预选与取消。V0.1.7 补充验收在同一受管后端事务入口执行真实切换和恢复，以验证新 stdin secret transport、Bridge-only restart、identity/revision 校验、单次真实请求和 Windows settings commit：

| 事务 | before revision | after revision | Bridge PID | 结果 |
|---|---|---|---|---|
| DeepSeek → OpenRouter | `162320-1787625164587295300` | `135900-1787631237919379600` | `156999` | `switchReturnedOk=true`；`stealth/ox-alpha`；路由请求 `success` |
| OpenRouter → 原 DeepSeek | `135900-1787631237919379600` | `166128-1787631265733624000` | `157675` | `switchReturnedOk=true`；`deepseek-v4-pro`；路由请求 `success` |

两次操作均由 `live_api_key_switch_diagnostic` 触发，每次只经产品 Bridge 发送一条 `max_tokens=1` 验证请求，两个测试进程退出码均为 0。Key 只在严格限长 JSON envelope 的 stdin 中进入 WSL；固定 argv 与 WSL `/proc/<pid>/cmdline` 均不含明文或 hex Key。Claude PID 始终保持 `108819`。

因此 B4/B7 的最新口径为：**UI 二次确认接线 LIVE；受管后端真实切换与恢复 LIVE+TEST；完整鼠标点击“确认切换”的视觉记录仍未补拍。**

### D7 真实出口补证

最终 feature-candidate debug 二进制执行 `--smoke`，真实 egress 为：

```text
PASS allow 431ms inputs=claudeRunning,windowsBridgePid wslInstalled=true distro=Ubuntu-24.04 linuxUser=lyuwinnie claudeRunning=true claudePid=108819 windowsBridgePid=none windowsBridgeProbe=checked runtimePresent=true listenerProbeOk=true listenerPresent=true daemonState=managed_ready pid8765=108819 pid8766=108819 controlSocket=true canOpen=true canStart=false
PASS paint 431ms budget=3000ms
PASS open 849ms login.url_ready loopback=true port=8765 nonce=present
PASS bridge 392ms health=200 models=200 identity=current modelCount=5
PASS egress 729ms work.bridge_egress.ok health=passed proxy=passed models=passed request=passed direct=skipped billable=true gating=false
PASS grade 3792ms state=running warnings=0 gating=false
SUMMARY required_pass=4 required_fail=0 non_gating_fail=0 elapsed_ms=6195 exit=0
```

因此 D7 的最新口径为：**UI 第一层费用说明/取消 LIVE；最终受管真实请求 LIVE+TEST；WORK 仍为 `gating=false`，没有成为启动条件。** UI 中“应用此修复”未在健康配置上强行点击，因为当前推荐值已经是 12334，且写操作只应来自确有故障时的用户确认。

### A–E 最新待人工项

- A2：维护窗口中的停止、启动、修复并重启；A7：旧 Windows Bridge fixture。
- B2：真实新增一条 Key；B3：真实手动模型列表/映射全流程；B6：真实聚合整表提交。
- D3：真实 DrvFS 授权修复；D7：故障现场中的“应用此修复”鼠标点击与 before/after 截图。
- E1–E4：正式 ZIP、SHA-256、包内 secret scan、双入口和并排升级。本轮明确不制作正式 V0.1.7 Release。

### 自动化最新结果

- Rust 完整测试：库 `107 passed / 0 failed / 4 ignored`；集成 `9 passed / 0 failed`；doc tests 0 项。
- 前端车道：`13 passed / 0 failed`；Vite build 35 modules。
- 总 self-test：54 translation tests；package policy passed；Python `35 passed / 3 skipped`；`self-test passed`。
- `cargo fmt --all -- --check` 与 `git diff --check` 通过。

S5 仍不写成“34 项全绿”；但此前最关键的 B4/B7 切 Key 事务和 D7 真实出口，已有真实、可回滚、已恢复原状态的补充证据。
