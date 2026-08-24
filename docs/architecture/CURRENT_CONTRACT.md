## 1. 第一原则：启动优先（本轮全部规则的来源）

**浮肿 ≠ 代码多。浮肿 = 启动的必要条件多。**

这是算术不是审美：设主路径有 n 个必要条件、每个单独成功率 p，整体可用率 ≈ pⁿ。
n 从 2 涨到 10，即使 p=0.98，可用率也从 96% 掉到 82%。
**加功能是加法，加必要条件是乘法。**

因此本指令**不限制**代码量、功能数量、模块数量，**只限制一件事**：谁能否决「打开 Claude Science」。

三条推论（后面全是它们的展开）：

1. **能力可以无限加，锁只有一把。** 新能力默认落 GRADE 或 WORK 车道。
2. **想给启动加锁要交税。** 税单见 §4.4，四件套缺一不可。
3. **不确定就放行。** 检测器拿不准时输出黄灯，不输出阻断。宁可给用户一个「能开但状态存疑」的系统，也不要给他一个「诊断很全面但打不开」的系统。

---

## 2. 产品句子（整轮不可改）

> CSA 是一个 Windows 控制面，管理唯一的 WSL Claude Science 运行时和当前包 Bridge；桌面 UI 负责启动、打开、配置 Key、观察和审批修复。深度体检、出口探针、存储迁移、运行时升级都是车间能力，不是点火前提。

CSA 不是模型平台，不是破解工具，不是全机体检仪。

---

## 3. 三条车道（本轮的核心结构）

| 车道 | 名字 | 能否禁用「打开 Claude Science」 | 失败时用户看到什么 | 何时跑 |
|---|---|---|---|---|
| **ALLOW** | 点火 | **只有这一套可以** | 分层错误，如 `login.socket_busy` | 首屏 + 用户点击 |
| **GRADE** | 仪表 | **永不** | 黄/红徽章 ＋「仍可打开」 | 首屏之后，后台周期 |
| **WORK** | 车间 | **永不**（用户点了修复且未完成时，只禁用修复按钮本身） | 折叠区里的进度与报告 | **仅用户点击** |

车道要落在**类型和 command 上**，不能只写文档：

- `AllowStatus` — distro / runtime 是否存在 / 受管 PID / 8765·8766 是否同主 / control socket 能否取 URL / `canOpen` / `canStart`
- `GradeStatus` — Bridge identity / 端口拓扑 / 出口浅层 / 代理 / 存储 / DrvFS 摘要 / warnings
- `WorkReport` — 深检 / 修复 / 迁移 Prompt / 升级 Prompt / 能力探针，**只在对应 command 的返回值里出现**

硬规则：

- GRADE/WORK 失败**不得**把 ALLOW 已验证的 PID / 端口填成空占位或降级默认值。
- `restartBlocked` 可以禁用「修复并重启」，**不可以**把一个已在监听 8765 的系统变成不能打开。
- 主按钮文案**只来自 ALLOW**，取值固定四条：`打开 Claude Science` / `启动 Claude Science` / `先停止旧 Windows Bridge` / `安装运行环境`。
  禁止出现 `先处理诊断问题`、无修饰的 `尚未启动`、任何来自 GRADE/WORK 的措辞。

---

## 4. ALLOW 冻结（收税口）

### 4.1 现状（已实测，不用重新调研）

`launcher/src/App.tsx:532`：

```ts
const canOpenClaude = Boolean(status.claudeRunning && !status.windowsBridgePid);
```

**已经只剩 2 个输入。** 早期那种「出口 / DrvFS / 磁盘全部 AND 进主按钮」的写法，在 `03dc6c2` 之前的几个 fix 里已经被拆掉了。

> ⚠️ **不要返工。** 不要重新去拆一遍已经拆好的东西。本轮对 ALLOW 只做一件事：**把这个 2 元集合焊死，防止它以后再长回去。**

### 4.2 冻结集合

| # | 输入 | 含义 |
|---|---|---|
| 1 | `claudeRunning` | 受管 Claude Science daemon 在跑，8765/8766 同 PID 持有 |
| 2 | `!windowsBridgePid` | 没有旧 Windows Bridge 抢占（否则形成双 Bridge） |

### 4.3 冻结测试 `allow_inputs_frozen`（强制执行点，必须存在）

```
断言 A：ALLOW 判据（canOpen）的输入名集合 == { claudeRunning, windowsBridgePid }
断言 B：主按钮文案（primaryLabel）的输入名集合 ⊆ ALLOW 字段
失败信息：ALLOW 输入集合被改动。改它要走 §4.4 的税单（改测试 + 改合同 + 反例测试 + 错误前缀）。
```

实现方式不限（Rust 常量枚举 + TS 同名常量对拍，或单侧枚举 + 快照），但必须做到：**有人给主按钮加第三个条件，测试立刻红**，而不是等用户打不开才发现。

**断言 B 不可省。** 只冻结 `canOpen` 不冻结文案，会漏掉这种情况：判据是干净的 2 元集合，但一块 GRADE 红徽章照样能把文案改成「先处理诊断问题」——**门锁住了，门牌还在被别人改。用户看到的是文案，不是布尔值。**

### 4.4 税单：想给 ALLOW 加第三个输入，同一个 commit 里交齐四件

1. 改冻结测试 `allow_inputs_frozen` 的期望集合；
2. 改 §4.2 的表，写明新输入的名字、含义、判定来源；
3. 附**反例测试**：证明「这个新条件误报时，一个本来能用的系统不会变成不能用」；
4. 写明错误前缀，落在 §6 的层名体系里。

**拿不出这四件的检查，一律降级为 GRADE。** 不接受「先加上，误判了再说」「多一层检查更保险」这类口头升级。

---

## 5. 降级不阻断（Degrade, never block）

GRADE 和 WORK 的任何失败，表达方式只有一种：**改徽章颜色和文字，不改主按钮。**

- 灰 = 没测到 / 超时熔断
- 黄 = 测到异常，但不影响使用
- 红 = 测到确定故障，**旁边必须有一句「仍可打开 Claude Science」**

**反向测试 `all_probes_red_button_still_open`（第二个强制执行点）：**

```
构造：把 GRADE/WORK 全部探针结果人为置为失败
      （出口 502、DrvFS 卡死、磁盘告警、canary 失败、代理冲突、identity 不匹配、深检超时……）
断言：① 主按钮文案不变
      ② 主按钮 disabled 不变
      ③ ALLOW 字段（claudeRunning / PID / 端口）不被任何失败结果覆写为空或降级占位
```

**熔断**：每个 GRADE/WORK 探针必须有硬超时 + 熔断，超预算即停、标灰、报 `*.timeout`。
禁止无限重试、禁止无限「正在处理…」、禁止把重试串成分钟级阻塞。

---

## 6. 错误必须带层名

| 前缀 | 含义 |
|---|---|
| `login.*` | ALLOW · 打开 / 登录 |
| `runtime.*` | ALLOW · 安装 / 启动 |
| `bridge.*` | 切 Key / runtime identity |
| `grade.*` | 仪表 |
| `work.*` | 车间深检 / 修复 / 能力探针 |
| `transport.*` | wsl.exe 运输层 |

示例：`login.socket_busy`、`runtime.lock_held`、`bridge.revision_mismatch`、`grade.sandbox_canary`、`work.bridge_egress.proxy_dead`、`transport.wsl_expansion`。

**禁止**再把 control-socket 轮换、CLI 裸退出码 2、canary 失败、DrvFS 卡死统统说成同一句「服务未启动」。同一层同一前缀，不同故障不同后缀。

---

## 7. 调用图与时间预算

| 用户动作 | 允许调用 | 禁止调用 | 预算 |
|---|---|---|---|
| 首次画出主按钮 | `get_allow_status`（或等价快路径） | 深检、VHDX 遍历、Git scan、MCP 预热、**任何阻塞式 `initialize_runtime`** | 主按钮可见 **< 3s**（WSL 已装） |
| 点「打开」且已在跑 | `open_claude_science` 轻量预检 + CLI 取 URL | `get_system_status` 全量、PyPI canary、DrvFS walk | **8s 内**给浏览器或分层错误 |
| 点「启动」 | start 事务（受管 current） | 深检、授权遍历 | 现有启动超时，不得叠加深检 |
| 30s 周期刷新 | **只更新 GRADE** | 写 ALLOW 的 PID / running / 端口 | 失败则徽章变灰 |
| 点「深度检测」 | WORK canary | 锁主按钮、改 PID | 独立 busy 标志 |
| 点「能力体检」 | WORK 能力探针 | 锁主按钮、改 PID、自动改系统 | 独立 busy 标志 |
| 确认切换 Key / 方案 | 预检 + Bridge-only 重启 + revision/identity 校验 | 重启 Claude daemon、打开登录页 | 失败回滚，禁止 `Ok(())` 早退 |

`initialize_runtime` 的定位：**可以在后台幂等地把服务拉起来，但不得挡住第一次 paint，不得在失败时用一次全量刷新冲掉登录错误。** 首屏先画出来，服务在后台自己起。

---

## 8. WSL 运输层冻结

只允许两种方式调 WSL：

1. `wsl.exe` + **参数数组**（每个参数独立传递，不经 shell）；
2. 经 **stdin** 把脚本送进 `bash`。

禁止：`bash -lc` 塞多行脚本、argv 里未保护的 `*`、未加引号的 `$()`。

新增任何 WSL 调用，必须附一条回归测试：**在「外层会展开」的假设下脚本仍然正确。** 运输层测试失败 = 产品失败，不是 flaky。

---

## 合同修订流程

修订必须是独立提交 `docs: revise CURRENT_CONTRACT vX.Y`，不得夹带功能改动；§4（ALLOW 冻结）和 §5（降级不阻断）是地基条款，修订需要人显式批准，模型不得自行放宽。
