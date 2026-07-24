# CSA Phase 2 核心技术栈与可行性测试

本文只确定关键技术路线和最小可行性测试，不要求一次性做成完整产品。当前建议把下一阶段拆成四个子功能：`API Key`、`Subagent`、`Connect`、`Research OS`。

## 0. 总原则

先验证关键链路，再封装产品界面：

1. 每个子功能先做一个可独立运行的最小测试。
2. 测试通过后再接入启动器 UI。
3. 默认只读和人工确认，尤其是外部 Agent、团队消息、迁移、环境安装。
4. 优先选择容易随便携包分发的技术栈，避免要求用户额外安装数据库、消息队列或云服务。

## 1. API Key

定位：模型供应商接入、Key 加密、模型发现、模型映射。

当前技术栈已经基本成立：

| 层 | 技术选择 | 状态 |
|---|---|---|
| 桌面 UI | Tauri 2 + React + TypeScript | 已存在 |
| 本地安全存储 | Windows DPAPI，绑定当前 Windows 用户 | 已存在 |
| 后端命令 | Rust Tauri commands | 已存在 |
| 运行时配置 | WSL `~/.claude-science/proxy/config.json` | 已存在 |
| 模型接入 | Anthropic-compatible / OpenAI-compatible | 已存在 |

最小可行性测试：

1. 新增两条同类中转配置时名称不重复。
2. 保存后不回显明文 API Key。
3. 切换 Key 后 Bridge 配置与 `/health` 一致。
4. 模型列表读取失败时仍允许用户手动填写模型。

结论：`API Key` 是第一块成熟模块，不是 Phase 2 的最大风险。

## 2. Subagent

定位：当沙盒内遇到数据集下载、环境安装、VM/SSH/GPU、迁移扫描等卡点时，把请求转交给宿主机外部 Agent，例如 Claude Code。

核心问题：沙盒内能不能把消息发给面板？

结论：可以，但不建议第一版让沙盒直接触发外部命令。推荐拆成两层：

1. 沙盒只提交任务请求。
2. 面板展示请求，由用户确认后再启动外部 Claude Code。

### 2.1 推荐技术栈

| 层 | 首选方案 | 备选方案 |
|---|---|---|
| 沙盒到面板通信 | 工作区文件收件箱 | localhost HTTP |
| 请求格式 | JSON 文件 / JSONL | HTTP JSON |
| 面板监听 | Tauri/Rust 轮询或文件 watcher | Rust localhost server |
| 外部 Agent 执行 | `claude -p <prompt>` | 未来支持 codex/其他 CLI |
| 结果保存 | `reports/csa-agent-runs/<runId>/` | SQLite |

### 2.2 为什么文件收件箱优先

文件收件箱更适合第一轮验证：

- 沙盒通常能写当前项目目录。
- 不依赖沙盒能否访问宿主机 localhost。
- 不需要新增本地端口和 token 体系。
- 用户容易理解：Agent 往一个目录放请求，面板读取请求。

建议路径：

```text
reports/csa-agent-inbox/<requestId>.json
reports/csa-agent-runs/<runId>/result.json
```

请求 schema 草案：

```json
{
  "schemaVersion": 1,
  "source": "sandbox",
  "taskKind": "dataset | environment | vm | migration | custom",
  "title": "数据集下载失败",
  "cwd": "C:\\path\\to\\workspace",
  "note": "只放脱敏报错和目标说明",
  "requestedAction": "diagnose",
  "createdAt": "2026-07-15T00:00:00Z"
}
```

### 2.3 localhost HTTP 何时使用

当文件收件箱验证通过后，再做 HTTP：

```http
POST http://127.0.0.1:<port>/api/subagent/requests
GET  http://127.0.0.1:<port>/api/subagent/requests
POST http://127.0.0.1:<port>/api/subagent/runs/:id/approve
```

HTTP 必须满足：

- 只绑定 `127.0.0.1`。
- 首次启动生成高熵本地 token。
- 请求体限流、大小限制、超时。
- 不接受外部网页跨域直接调用。
- 默认只入队，不自动执行。

最小可行性测试：

1. 在沙盒内写入一个 `request.json`。
2. 面板能读到请求并显示任务类型。
3. 用户点击确认后，面板调用本机 `claude`。
4. stdout/stderr 写入结果区和 `result.json`。
5. 请求和结果都不包含 API Key、token、完整 `.env`。

结论：`Subagent` 的核心技术栈应先验证“文件收件箱 + 手动批准 + Claude Code 执行”。

## 3. Connect

定位：团队协作连接层，不是第一版的复杂团队管理系统。先做消息出入口，再做权限、成员、项目管理。

建议把 Connect 拆成三层：

| 层 | 作用 | 第一轮做法 |
|---|---|---|
| Outbound | CSA 向团队工具发通知 | Webhook |
| Inbound | 团队工具向 CSA 发命令 | 暂缓或走云端 relay |
| Audit | 记录谁触发了什么 | 本地 JSONL/SQLite |

第一批适配对象：

1. 飞书群机器人 Webhook。
2. 企业微信 / 微信群机器人 Webhook。
3. 通用 Webhook。

安全边界：

- 第一轮只做 outbound 通知，不做远程执行。
- 不把本地 CSA 端口暴露到公网。
- 不通过聊天工具传 API Key、完整日志、完整 Prompt。
- 如果未来要 inbound，建议通过云端 relay 做鉴权和审计，不让用户路由器开端口。

最小可行性测试：

1. 面板生成一条脱敏通知。
2. 发送到一个测试 Webhook。
3. 失败时记录 HTTP 状态和错误摘要。
4. 通知内容只包含任务状态、runId、短摘要、打开本地面板的提示。

结论：`Connect` 第一阶段只做“脱敏通知通道”，不要先做完整团队系统。

## 4. Research OS

定位：学术流利度、知识飞轮、科研工作流评估。它不是先做一个大模型打分器，而是先建立可观测事件流。

核心判断：没有结构化事件，就无法稳定评价“用得怎么样”。

建议技术栈：

| 层 | 技术选择 |
|---|---|
| 事件记录 | JSONL，后续可迁 SQLite |
| 事件来源 | CSA 状态、Subagent 请求、Agent 结果、API Key 连通、环境检查 |
| 指标计算 | 本地 TypeScript/Rust 规则引擎 |
| 模型评估 | 第二阶段可选，用外部模型生成建议 |
| 展示 | 面板中的 Research OS 模块 |

第一版不要记录完整对话内容，只记录结构化事件：

```json
{
  "eventType": "agent_run_completed",
  "projectId": "local-project",
  "taskKind": "environment",
  "durationMs": 120000,
  "ok": true,
  "blockedReason": null,
  "createdAt": "2026-07-15T00:00:00Z"
}
```

可先评估四类流利度：

1. 环境流利度：环境检查是否一次通过、失败后是否能定位。
2. 数据流利度：数据集下载是否可恢复、是否有校验。
3. Agent 流利度：卡点是否能从沙盒转交外部 Agent，并回写结果。
4. 研究流利度：任务是否形成可复用记录、结论、下一步。

最小可行性测试：

1. 写入 10 条脱敏事件。
2. 生成一个本地评分 JSON。
3. 输出“当前最大阻塞项”和“下一步建议”。
4. 不依赖模型也能给出基础分。

结论：`Research OS` 的第一阶段是事件系统，不是复杂评分模型。

## 5. 四模块的推荐落地顺序

| 顺序 | 模块 | 原因 |
|---|---|---|
| 1 | API Key | 已基本成熟，只需收尾 |
| 2 | Subagent | 直接解决沙盒卡点，是当前最大价值 |
| 3 | Research OS | 依赖 Subagent/状态事件，先做事件流 |
| 4 | Connect | 先做通知，等内部事件稳定后再接团队工具 |

## 6. 下一步最小实验

建议下一步只做一个实验：`Subagent 文件收件箱`。

实验目标：

1. 新增一个脚本写入 `reports/csa-agent-inbox/demo-request.json`。
2. 面板或命令行读取该请求。
3. 人工确认后调用 `claude -p`。
4. 把结果写入 `reports/csa-agent-runs/demo/result.json`。

如果这条链路跑通，说明“沙盒消息进入面板，再由宿主机外部 Agent 处理”的关键技术路线成立。之后再决定是否封装成 UI tab、localhost API、团队通知和 Research OS 事件。

## 7. Phase 2 Subagent Hub v0 构建记录

第一版已经按“文件收件箱 + 手动批准 + Claude Code 执行 + 结果回写”实现最小闭环：

| 能力 | 实现 |
|---|---|
| 写入 demo 请求 | `scripts/create-subagent-request.ps1` |
| 面板读取收件箱 | Tauri command `list_subagent_requests` |
| 面板创建 demo | Tauri command `create_demo_subagent_request` |
| 手动批准运行 | Tauri command `run_subagent_request` |
| 外部 Agent | 复用 `claude -p <prompt>` |
| 结果回写 | `reports/csa-agent-runs/<runId>/result.json` |

当前版本仍保持人工确认：

- `approvalMode=manual` 时必须用户点击“批准运行”。
- `approvalMode=autoCandidate` 只作为未来自动策略字段预留，当前不会自动执行。
- 运行结果写入 `request.json`、`prompt.md`、`stdout.txt`、`stderr.txt`、`result.json`。
- `reports/csa-agent-inbox/` 和 `reports/csa-agent-runs/` 已加入 `.gitignore`，避免本地请求和结果误提交。

命令行投递示例：

```powershell
.\scripts\create-subagent-request.ps1 `
  -TaskKind dataset `
  -Title "Dataset download diagnosis" `
  -Note "Read-only: please plan host-side checks before download."
```

验收边界：

1. 脚本能写入 `reports/csa-agent-inbox/<requestId>.json`。
2. 面板能刷新看到该 request。
3. 点击“批准运行”后才调用本机 Claude Code。
4. 完成后生成 `reports/csa-agent-runs/<runId>/result.json`。
5. 当前版本不执行自动批准策略、不远程审批、不开放 localhost HTTP。

## 8. Phase 2 Subagent Hub v1 轻量会话路线

用户希望的目标不是只做一次性 CLI 调用，而是形成一个轻量任务中枢：沙盒把任务投递出来，面板批准运行，外部 Claude Code 返回结果后还能继续对同一会话发送消息。

参考方向：

- `multica-ai/multica` 更像完整任务平台和多 Agent runtime，能力完整但第一阶段偏重。
- `Aster110/cc2wechat` 更接近轻量桥接：外部消息进入本机 daemon，再路由到 Claude Code 会话。
- CSA 当前采用中间路线：文件收件箱 + 手动批准 + Claude Code `session_id` + `claude --resume <sessionId> -p <message>`。

v1 最小实现：

| 能力 | 实现 |
|---|---|
| 记录会话 | 解析 `claude -p --output-format json` 返回的 `session_id` |
| 继续会话 | 调用 `claude --resume <sessionId> -p <message>` |
| 面板交互 | 在 Subagent Hub 显示 session、result、继续发送框 |
| 结果留痕 | 每次续跑写入 `reports/csa-agent-runs/continue-*/result.json` |
| 安全边界 | session id 只允许安全 ASCII token；默认仍需用户手动发送 |

自动模式预留：

1. `approvalMode=manual`：默认模式，用户点击后执行。
2. `approvalMode=autoCandidate`：未来自动模式候选，只入队不执行。
3. `policyId`：未来按任务类型、目录、命令风险、预算限制决定是否可自动执行。

结论：第一阶段不做嵌入式终端、不模拟键盘、不注入 Claude Code 交互框；先验证 `session_id + --resume` 是否足够支撑“任务对话可继续”。如果这条链路稳定，再评估是否需要 PTY/WebSocket/daemon 化的实时终端。
