# CSA v0.1.8 双皮肤 UI 功能候选验收（2026-08-25）

## 结论与边界

P0 与 P1 已严格分开：P0 复盘、长期开发手册和错误清单先在提交 `4541841` 完成，确认 `App.tsx` / `App.css` 零改动后，才开始 P1。P1 初版的唯一设计依据是 `docs/prompts/CSA_v018_UI_PROMPT.md` §P1.3 的纯文字线框；没有读取 `docs/design/`，也没有用外部网页补充设计。初版验收后，用户在本任务中明确要求新版改为亮堂的极简配色、宽度相对自适应，并让 Classic 四个主控键等尺寸；该直接要求只覆盖视觉 token 与布局，不扩大后端范围。

P1 代码已完成同一 DOM 的 `console` / `classic` 双皮肤、首次选择和设置持久化、状态屏、独立 F1/F2/F3、接入列表改造、两步删除、任意 Provider 自定义名称和重命名。既有 ALLOW、GRADE/WORK 探针、Bridge 事务、Provider 预设和旧 Tauri command 签名均未改；只新增 `get_ui_preferences`、`save_ui_skin`、`rename_api_key` 三个设置类 command。任务书“smoke 六项与改造前一致”的外部出口现场证据尚未全绿，见“自动化与不变量”，因此不能把本报告写成无条件正式验收完成。

这是 **v0.1.8 feature-candidate 源码与 debug 验证产物**，不是正式 Release。界面候选标识为 `v0.1.8`；`package.json`、Cargo 和 Tauri 发布元数据仍保持 `0.1.6`，待正式发布门统一升级。本轮没有制作 Release ZIP/SHA-256，没有关闭当前启动器、Claude Science 或 WSL，也没有触碰 2222 等无关服务。

## P0 教训如何进入 P1

最严重的问题不是某一个端口，而是连续补丁阶段没有先冻结“谁有权决定可打开”，把 ALLOW、WORK 探针、修复动作和界面文案混在一起解释，导致一次非门禁探针超时或受管入口差异都可能被翻译成“Claude Science 没启动”，形成反复修一处、另一处又改口径的鬼打墙。v0.1.6/v0.1.7 在短时间内连续 12 次修补、`lib.rs` 非空行从 7086 增至 8876，是这一失序的量化信号。

P1 因此遵守四条硬规则：只有 ALLOW 决定主按钮；GRADE/WORK 只显示证据和建议；皮肤只改变呈现、不复制业务 DOM；设置写入不触发 Bridge commit/restart。代码变更前先冻结旧测试和函数合同，改完再以源码等值、完整回归和现场预览三种证据交叉验收。

## 实现结果

### 1. 双皮肤与首次选择

- 页面只有一个 `<main>`、一套控制台、一套接入管理 DOM；根节点和 `documentElement` 仅通过 `data-skin="console|classic"` 切换样式。
- 首次没有 `ui_skin` 时显示皮肤选择器，并把后面的应用设为 `aria-hidden` + `inert`；选择写入 `LauncherSettings`，不使用 `localStorage`。
- 读取失败、字段类型错误或非法值统一回退到 `console`，不会出现空白页。合法 `console` / `classic` 可往返保存。
- 真实跨进程证据：以全新隔离 APPDATA 启动当前 debug Tauri，首次选择页出现；通过界面选择 Classic 后，隔离 `settings.json` 为 `uiSkin=classic`、`apiKeys=[]`。只关闭该测试窗口并以同一 APPDATA 重启，选择页不再出现，Classic 浅色界面和默认展开诊断直接恢复。整个过程未读写真实 LauncherSettings、Key 或服务状态。
- Console 改为明亮的白/浅绿仪器配色，保留 warning/fault 的琥珀与锈红层级、中文字体回退和 `prefers-reduced-motion`。主体宽度为 `clamp(640px, 88vw, 1040px)`，再受窗口安全边距约束；健康卡用 `auto-fit`，较窄窗口中的运行时和新增接入自动退回单列，不再固定 680px 挤满内容。
- Classic 保留原有浅色信息架构，并把主控、F1、F2、F3 放进同一个四等分网格：四键同宽、同高（68px）和同圆角；900px 以下自动变为两列。
- 本地预览逐项比较：两种皮肤均有 23 个按钮，按钮文字集合完全相同；Console 诊断默认收起，Classic 默认展开。隔离 debug Tauri 又实测了亮色 Console 的相对宽度和 Classic 四等分控制键，未见横向溢出。预览/实测只验证实现，不作为额外设计来源。

### 2. 状态屏与主按钮

- 启动读数每 3 秒推进一行、最多保留 40 行，可暂停，使用 `aria-live="polite"`；只有状态可读后才挂载读数区，不运行常驻 animation frame。
- 警告为 amber；已确认故障为 rust，紧跟 `仍可打开 Claude Science`。屏幕去掉内部错误码前缀，完整原始诊断仍在折叠区。
- 主按钮继续直接使用既有 `primaryButton.label` / `primaryButton.disabled`；屏幕消息、动画、折叠状态不写 ALLOW，也不改变主按钮。
- F1“深度检测”、F2“能力体检”、F3“修复并重启”各用自己的 busy 状态。F2 保留 `max_tokens=1` 费用确认；F3 继续走既有 repair 入口，不冒充 ALLOW 操作。

### 3. 接入管理

Provider 目录的前后快照完全一致：前后函数正文均为 5013 字符，SHA-256 均为 `687ba3d8b36b08875329a309c0a76a571f35d39dd47d2f66b8f631cb242d8399`。

| 层级 | Provider（顺序冻结） |
|---|---|
| 官方直连 / `official` | GLM-5.2、LongCat、DeepSeek、MiniMax、Claude、OpenAI / GPT |
| 聚合与编程订阅 / `aggregator` | OpenCode Go、OpenRouter |
| 中转服务 / `custom` | 项目方自建中转、自定义中转 |

- “我的接入”显示全部已保存项、Provider、脱敏密钥/官方登录、决策模型及 active/pending 状态；单 API 与两套三模型聚合方案仍共存。
- 新增流程保留“测试 → 自动匹配 → 重新获取模型 → 手调三角色映射”；中转域名确认、单 Key 预选/二次确认、聚合整表一次提交和失败回滚逻辑不变。
- 任意 Provider 都能填写 1–80 字符名称，并与重命名共用后端校验。GLM 可保存为“实验室备用”；普通 Provider 留空回退 Provider 名称，自定义中转留空回退日期序号名；空白重命名、换行及超过 80 字符会拒绝。
- 删除必须先点一次“删除”，再显示 `确定删除「<接入名称>」？删除后无法恢复。`；“取消”默认获得焦点，取消后焦点回到原删除按钮。第一击不调用删除 command；活动单 Key或当前聚合方案引用项禁删；同一时刻只允许一项待确认；没有 `window.confirm`。

## A–E 能力核对

证据标记：`PREVIEW` 表示在本地 Vite 预览中逐项交互，只证明界面接线；`TEST` 表示自动化或源码等值合同；`待人工` 表示会改变服务、凭据、计费或正式发布状态，本轮没有代点。

| 车道 | P1 后仍可达的能力 | 结果与证据 |
|---|---|---|
| A 点火 | 刷新；主启动/打开；页脚停止、修复并重启；旧 Bridge 条件入口；Runtime Update 检查、升级/回退 Prompt | 双皮肤同 DOM `PREVIEW+TEST`；主按钮仍只绑定 ALLOW。打开链路由 smoke 通过；停止/修复/旧 Bridge fixture 为 `待人工` |
| B 接入 | 10 项目录；新增/测试/模型获取/自动匹配/手调；单 Key 预选+确认；方案一/二与整表提交；列表、启用、重命名、两步删除 | 目录等值 `TEST`；GLM 命名、测试、自动映射、重新获取、待启用、重命名、删除取消及两套方案均已 `PREVIEW`；真实新增 Key、确认切换和聚合提交为 `待人工` |
| C 仪表 | WSL、运行时、Bridge、Claude、出口、存储、当前接入；完整诊断与维护 | 两皮肤逐项可见 `PREVIEW`；诊断折叠不门禁，GRADE 仍 `gating=false`，完整 Rust 回归 `TEST` |
| D 车间 | F1 深检、F2 出口能力体检、F3 修复；存储建议；版本检查/升级/回退 Prompt；出口修复 Prompt/应用入口 | F1/F2/F3 独立 busy `TEST`；F2 费用说明和取消、存储与版本 Prompt 已 `PREVIEW`；真实修复、出口应用及计费请求除 smoke 一次外为 `待人工` |
| E 交付 | Node/Rust/build/self-test、包策略、双入口与并排升级合同 | 自动化 `TEST` 通过；本轮明确不做正式 ZIP/SHA、包内 secret scan 或并排接管，均为发布门 `待人工` |

预览补充证据：首次选择时底层 DOM 确实为 `aria-hidden="true" inert`；Console 宽度为 680px；状态未 ready 时读数区数量为 0；展开诊断后逐项核对维护入口；添加窗口显示 3 层 10 项 Provider 和完整四步；GLM 使用本地 dummy 值测试成功，名称“实验室备用”进入待启用列表并可重命名；删除第一击卡片仍为 1 项，确认文本准确，“取消”有焦点，取消后卡片仍在且焦点返回“删除”。没有执行最终删除。跨进程记忆另由上述隔离 Tauri 重启实测证明，不以 Vite fixture 代替。

## 自动化与不变量

```text
Node lanes:          21 passed, 0 failed
Vite build:          passed, 36 modules
Rust lib:           113 passed, 0 failed, 4 ignored
Rust integration:     9 passed, 0 failed
self-test:             54 translation tests passed
Python/package:        35 passed, 3 skipped; package policy passed
cargo fmt:             passed
git diff --check:      passed
```

冻结核对：

- `allow_inputs_frozen` 仍严格只接受 `claudeRunning`、`windowsBridgePid`；既有 all-probes-red 期望未修改。
- Provider 目录函数逐字节等值；`laneContract.ts`、`bridge_egress.rs`、`bridge_egress_apply.rs`、`runtime_lifecycle.rs` 和三个既有 Node 合同测试未改。
- `allow_status_impl`、Bridge verify/commit/activate 事务段与 P0 HEAD 等值；保存皮肤与重命名只写 LauncherSettings。
- 新 UI 回归覆盖 `skin_choice_persists`、`both_skins_keep_all_entries`、`delete_requires_two_steps`、`delete_confirm_names_target`、`custom_label_accepted_for_any_provider`、`screen_never_gates_main_button`、F1/F2/F3 独立 busy、亮色自适应 token、Classic 四等分控制键与 reduced motion。屏幕源码切片还直接禁止调用四个 ALLOW setter/commit 入口；运行时非门禁结论仍以既有 all-probes-red 为权威。

最终独立 debug `--smoke`：

```text
PASS allow 422ms ... canOpen=true
PASS paint 422ms budget=3000ms
PASS open 846ms loopback=true port=8765 nonce=present
PASS bridge 431ms health=200 models=200 identity=current modelCount=5
FAIL egress 1428ms work.bridge_egress.upstream_http ... billable=true gating=false
PASS grade 3413ms state=running warnings=0 gating=false
SUMMARY required_pass=4 required_fail=0 non_gating_fail=1 elapsed_ms=6542 exit=0
```

本次 egress 在用户已配置的受管路径发送了一条 `max_tokens=1` 后取得 `work.bridge_egress.upstream_http`；它是当前上游 HTTP 结果，`gating=false`，不证明代理或 Claude Science 启动失败，也不改变主按钮。为避免重复计费，本轮没有重跑。

## 任务书验收六问

1. **P0 最严重的错误是什么，后续规则是什么？** 错在没有先冻结唯一门锁和合同，边探测、边改门禁、边重写解释，造成非门禁故障被误报成未启动。后续固定“三车道 + 唯一 ALLOW”，P0/合同/基线先行，设置/UI 不得暗改生命周期。
2. **`allow_inputs_frozen` / all-probes-red 是否被改？** 没有。既有测试文件和期望原样保留；完整 Rust/Node 回归通过，主按钮源码仍只绑定 `primaryButton`。
3. **Classic 是否保留 A–E 全部能力？** 保留。两皮肤共享同一 DOM；现场比较为 23 个按钮对 23 个按钮且文字集合相同。A–E 入口见上表，未执行的破坏性项没有冒充已完成。
4. **屏幕红字会不会改变主按钮？** 不会。红字由诊断副本派生，只改屏幕行；后面明确显示“仍可打开 Claude Science”。主按钮继续由 ALLOW 生成，all-probes-red 与新屏幕合同共同冻结该边界。
5. **第一次点删除会不会真删，确认是否点名？** 不会。第一击只设置待确认 ID；确认行逐字显示目标名称和“删除后无法恢复”，取消默认聚焦并恢复原按钮焦点。自动测试和本地取消实测均通过。
6. **GLM 能否叫“实验室备用”，空名称如何处理？** 可以，且已在预览中保存并重命名。普通 Provider 空名称回退官方 Provider 名；自定义中转空名称回退日期序号名；1–80 字符校验由保存与重命名共用。

## 未代做的项目

- 维护窗口中的真实停止/重启/持久 DrvFS 授权修复，以及旧 Windows Bridge fixture。
- 使用用户真实 Key 新增、测试、确认切换、最终删除和真实聚合整表提交。
- 在故障现场点击出口“应用此修复”；本轮只验证 Prompt/入口，没有制造错误配置。
- 正式 v0.1.8 版本元数据、Release ZIP、SHA-256、包内 secret scan、BAT/PowerShell 双入口与并排升级。进入发布门时必须一次性同步版本并重新做 E 车道验收。
