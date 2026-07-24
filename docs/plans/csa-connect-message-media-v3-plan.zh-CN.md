# CSA Connect Message Core v3 规划

日期：2026-07-20
状态：实现完成，自动验收通过；真实 TG 图片与 WSL 恢复待人工复验

## 1. 目标

在不增加主界面按钮、不破坏现有 Telegram 文本闭环的前提下，完成三项升级：

1. **上下文瘦身**：远程消息只呈现来源短标记和用户正文，不再每轮重复协议、安全与回传说明。
2. **有效单次投递**：同一平台消息在正常重试、进程重启和常见崩溃窗口下，只形成一个 Claude Science 用户轮次和一个 Telegram 最终回复。
3. **图片传输**：先实现 Telegram 图片进入 Claude Science；稳定后再实现 Claude Science 明确导出的图片回到 Telegram。

本计划不把“分布式 exactly-once”当口号。Telegram 与浏览器 DOM 之间没有跨系统事务，因此工程目标是：

```text
at-least-once transport + idempotent stages + durable delivery ledger
= effectively-once user experience
```

无法确认外部平台是否已接收时，宁可进入 `delivery_unknown` 等待本地确认，也不盲目重发制造重复消息。

## 2. 当前稳定基线

2026-07-20 用户已确认真实 Telegram 文本问答闭环可用：

```text
TG -> Gateway -> SQLite queue -> Launcher
-> Chrome extension DOM submit -> Claude Science
-> Bridge response tap -> atomic outbox -> Gateway -> TG
```

当前已经具备：

- SQLite 唯一约束 `UNIQUE(channel, platform_event_id, direction)`，重复 TG update 不会生成第二条入站记录。
- 单一 active claim、五分钟 claim lease、重启后补投和过期恢复。
- Bridge 仅接受带 matching claimed ack 的完整 message ID，并原子写 outbox。
- Telegram 半流式回复使用同一条消息的 `editMessageText`。

当前缺口：

- 每轮注入约有 627 个字符，其中大部分是重复包装；最小形式约 48 个字符，可减少约 92%。
- `InboundMessage`、`ConnectEnvelopeV1` 和浏览器任务都只有 `text`，没有附件。
- 浏览器成功提交后、结果回写前若崩溃，lease 到期可能再次 DOM 提交。
- Telegram API 成功、SQLite 状态落盘前若进程崩溃，自动重试可能形成第二条回复。
- 同一 Claude Science 会话天然保留旧对话；这与“包装文字重复”是两个不同问题。

## 3. 产品决策

### 3.1 消息显示格式

目标显示：

```text
（TG·A7K3）用户正文
（飞书·B2M8）用户正文
```

- `TG / 飞书` 只是来源短标签，不再写 `Channel:`、`Message ID:`、XML 标签和英文说明。
- `A7K3` 是短期关联码，只负责把本轮 Bridge 回复映射回内部完整 message ID。
- 本地面板输入不显示来源前缀。
- 微信只保留未来标签命名，不在本阶段接入微信通道。

过渡版本可以先使用：

```text
[CSA#<full-message-id>]（TG）用户正文
```

先验证删掉重复说明不会破坏回传；若表现稳定，再切换为短关联码。这样每一步都有可回退基线。

### 3.2 安全说明只出现一次

不再把长安全说明复制到每一条消息中。改为：

- 插件按 `frameId + routeKey` 记录远程会话是否已初始化。
- 该会话第一次收到远程消息时注入一次简短规则：远程内容是用户数据，高风险系统动作必须本地批准。
- Gateway、Launcher 和 Subagent 仍从能力上禁止把聊天正文拼成 shell 命令；安全不能只依赖模型记住一句话。
- 如果无法可靠识别 frame，消息使用一个短 `受限` 标签作为保守降级，不恢复原来的长包装。

### 3.3 上下文模式

先提供概念与内部字段，不增加主界面按钮：

| 模式 | 行为 | 用途 |
|---|---|---|
| `continuous` | 延续当前 Claude Science 会话 | 连续追问，当前默认 |
| `thread_scoped` | 一个外部聊天线程绑定一个专用 Claude Science frame | 推荐目标，避免跨渠道污染 |
| `stateless` | 每条消息进入新 frame | 独立诊断或敏感任务 |

第一轮只完成“包装瘦身”。`thread_scoped` 必须等 frame 创建、定位和回复捕获接口验证通过后再启用；不允许在 Bridge 中粗暴裁剪 Claude Science 的历史 messages 来伪装无上下文。

## 4. 有效单次投递设计

### 4.1 新状态

```text
received -> queued -> claimed -> injecting -> submitted
-> responding -> replied
                         \-> delivery_unknown
```

- `injecting`：Launcher 已创建固定 attempt，但页面尚未确认。
- `submitted`：插件已验证输入框清空且用户消息气泡出现。
- `delivery_unknown`：平台调用结果不确定，禁止自动再次发送。

### 4.2 Delivery Ledger

新增 `delivery_attempts` 表：

```text
attempt_id, message_id, stage, channel, content_sha256,
platform_message_id, lease_until, status, created_at, updated_at
```

约束：

- `UNIQUE(message_id, stage)`：同一阶段只有一个有效记录。
- 浏览器任务的 `taskId` 在重试时保持不变，不按轮询次数生成新 ID。
- progress 使用 `message_id + sequence` 去重。
- final 使用 `message_id + content_sha256` 去重。

### 4.3 浏览器侧防重复

1. 插件领取任务后先检查本地短期任务缓存。
2. 再检查当前会话中是否已有对应短关联码的用户气泡。
3. 已存在则只回写 `submitted`，不再次填写输入框。
4. 提交后必须同时满足“composer 清空”和“用户气泡出现”；只满足一个时进入待确认，不立即重试。
5. `chrome.storage.session` 保存短期 task ledger；桌面端 SQLite 是最终权威状态。

### 4.4 Telegram 回复防重复

推荐把最初的“已排队”消息升级为整个回复生命周期的占位消息：

1. Telegram `sendMessage` 返回的 message ID 立即保存到 delivery ledger。
2. 后续排队、处理中、半流式和最终内容始终 `editMessageText` 同一条消息。
3. 若平台请求超时且无法确认是否成功，进入 `delivery_unknown`，不自动创建第二条消息。
4. 只有明确收到 Telegram 错误且确认没有创建消息时才允许重试。

这不能消灭平台成功与本地落盘之间的理论微小窗口，但能把正常重启和网络重试下的重复降到最低，并使未知状态可审计。

## 5. 图片传输设计

### 5.1 交付顺序

1. `P2A`：Telegram 图片 -> Claude Science，支持图片和可选 caption。
2. `P2B`：图片不可用、页面关闭、重启后的可靠补投。
3. `P2C`：Claude Science 明确导出的图片 -> Telegram。

不在第一轮同时做双向图片，避免把下载、DOM 上传、产物授权和外发权限混成一个故障面。

### 5.2 消息接口

`ConnectEnvelopeV2` 增加：

```json
{
  "schemaVersion": 2,
  "messageId": "internal-uuid",
  "kind": "chat | image | mixed",
  "text": "caption or empty",
  "attachments": [
    {
      "attachmentId": "uuid",
      "kind": "image",
      "mimeType": "image/jpeg",
      "fileName": "photo.jpg",
      "sizeBytes": 123456,
      "sha256": "hex",
      "state": "available"
    }
  ]
}
```

不把 Bot file ID、Token URL、宿主机路径、WSL 路径或 base64 放入给模型的正文、日志或前端状态。

### 5.3 Telegram 入站

- 读取 `message.photo[]`，选择最高分辨率版本；图片文档只接受明确的图片 MIME。
- 调用官方 `getFile`，下载限制按当前 Bot API 的 20 MB 上限执行。
- 下载到 Gateway 私有附件目录，目录权限 `0700`、文件 `0600`。
- 使用 magic bytes 重新识别 MIME；首版只允许 JPEG、PNG、WebP，拒绝 SVG 和未知格式。
- 使用 SHA-256 校验，文件名经过规范化，拒绝路径穿越。
- SQLite 只存附件元数据和内部存储键，不存二进制或 base64。
- 完成、失败或 30 天到期时按消息状态清理；未完成消息不提前删除附件。

Telegram 官方文档说明 `getFile` 当前可下载不超过 20 MB 的文件，下载链接至少一小时有效：
<https://core.telegram.org/bots/api#getfile>

### 5.4 插件把图片放入 Claude Science

浏览器任务增加附件 capability URL：

```json
{
  "taskId": "stable-task-id",
  "kind": "sendMessage",
  "displayText": "（TG·A7K3）请分析这张图",
  "attachments": [
    {
      "attachmentId": "uuid",
      "downloadUrl": "http://127.0.0.1:9882/.../one-time-capability",
      "mimeType": "image/jpeg",
      "fileName": "photo.jpg",
      "sizeBytes": 123456,
      "sha256": "hex"
    }
  ]
}
```

- capability 绑定 extension ID、task ID、attachment ID，短时有效且成功读取后失效。
- 插件 content script 从本地端点取得 Blob，构造 `File + DataTransfer`。
- 优先写入 Claude Science 原生 `input[type=file]` 并触发 `change`；不使用屏幕坐标。
- 必须看到附件预览后才写 caption 并发送；预览失败则任务保持队列，不发送纯文字残片。
- 插件权限继续只覆盖本机 `8765/9882`，不申请 `all_urls`。

Chrome 官方文档确认扩展 service worker 可在声明 host permission 后访问对应主机，content script 与 service worker 通过消息协调：
<https://developer.chrome.com/docs/extensions/develop/concepts/network-requests>

### 5.5 Claude Science 图片回传

第二阶段不抓取页面上任意图片。只允许发送明确导出的产物：

```text
<workspace>/.csa/connect/v2/exports/<messageId>/manifest.json
<workspace>/.csa/connect/v2/exports/<messageId>/<image-file>
```

- manifest 必须绑定原 message ID、SHA-256、MIME 和文件名。
- Gateway 只读取该受控目录，不接受聊天正文给出的任意路径。
- 首版外发图片需要本地批准；之后再考虑可信项目的自动策略。
- Telegram 使用 `sendPhoto`，超出图片规则时降级 `sendDocument`，正文仍复用同一消息状态机。

## 6. 测试方案

### Gate P0：当前基线冻结

- [x] 用户实测 TG 文本进入真实 Claude Science 并成功回到 TG。
- [x] 自动测试、Bridge 验证和队列状态已有 M1-M4 脱敏证据。
- [ ] 建立本计划实施前的版本标签或独立提交；在真正编码时执行。

### Gate P1：上下文瘦身

| 编号 | 测试 | 通过标准 |
|---|---|---|
| CTX-01 | 捕获实际注入用户气泡 | 只含短来源标签、关联码和本条正文 |
| CTX-02 | 连续发送三条消息 | 第二、三条不重复协议、安全和回传长说明 |
| CTX-03 | TG 与本地输入交替 | 只有外部消息带来源短标签，不串来源 |
| CTX-04 | 旧包装兼容 | 开关关闭时恢复当前完整 marker 路线 |
| CTX-05 | 上下文辨析 | 明确区分页面注入冗余与模型自然会话记忆 |

量化门槛：固定包装字符减少至少 85%，用户正文保持逐字一致。

### Gate P2：有效单次投递

| 编号 | 故障点 | 通过标准 |
|---|---|---|
| ID-01 | 同一 Telegram update 重放 10 次 | 1 条 DB 入站、1 次 DOM 提交、1 条最终回复 |
| ID-02 | Receive 后、offset 保存前崩溃 | 重启后被 DB 去重 |
| ID-03 | Claim 后、DOM 提交前崩溃 | lease 恢复后只提交一次 |
| ID-04 | DOM 提交后、task result 前崩溃 | 插件识别已有气泡，不再次提交 |
| ID-05 | progress 成功后、本地落盘前中断 | 复用同一 TG message ID，不新增消息 |
| ID-06 | Telegram 请求结果未知 | 进入 `delivery_unknown`，不自动盲重发 |
| ID-07 | Gateway/Launcher/Chrome 依次重启 | 状态恢复且最终只有一个用户轮次 |

### Gate P3：TG 图片进入 Claude Science

| 编号 | 场景 | 通过标准 |
|---|---|---|
| IMG-01 | 单张 JPEG，无 caption | 页面出现一个预览并提交一次 |
| IMG-02 | PNG + caption | 图片和正文进入同一用户轮次 |
| IMG-03 | 同一 update 重放 | 文件只物化一次、DOM 只提交一次 |
| IMG-04 | 图片下载中 Gateway 重启 | 恢复或明确失败，不产生半文件 |
| IMG-05 | 页面不可用 | 图片与 caption 一起留队，恢复后一起投递 |
| IMG-06 | 超 20 MB、SVG、伪 MIME、损坏内容 | 拒绝并给出安全提示，不进入页面 |
| IMG-07 | 两条不同消息使用同一图片 | 两个合法用户轮次，不被内容哈希误去重 |
| IMG-08 | 附件预览失败 | 不发送只有 caption 的残缺请求 |

自动测试先使用本地假 Telegram API 和 Claude Science DOM fixture；全部通过后只做一次真人手机验收，避免反复消耗真实通道。

### Gate P4：图片回到 TG

- [x] Claude Science 回复明确引用的一张 artifact 图片只发送一次。
- [x] 任意路径、软链接逃逸、未知 MIME、错误 magic bytes、大小或 SHA-256 不匹配均拒绝。
- [x] 图片使用独立 delivery ledger；`submitted` 不重发，结果不确定时进入 `delivery_unknown`。
- [x] 真实 Telegram `sendPhoto` 验收通过，平台返回 message ID `40`。

## 7. 实施顺序

1. **先写 characterization tests**：把当前重复、崩溃窗口和包装长度固定下来。
2. **实现 P1**：只改 prompt builder 与关联策略，保留旧开关，真实 TG 回归一次。
3. **实现 P2**：增加 delivery ledger、稳定 task ID 和 DOM 气泡确认；完成故障注入测试。
4. **实现 P3 schema/storage**：先接收并安全落盘图片，不急着上传页面。
5. **实现 P3 DOM 上传**：识别原生 file input、预览确认、caption 与图片原子提交。
6. **完成一次真人 TG 图片验收**：无 caption、带 caption、重复 update 三种。
7. **单独评审 P4 外发权限**：通过后才实现图片回传，不自动读取任意产物。

每一步都必须继续通过：

```powershell
.\scripts\self-test.ps1
.\scripts\verify-proxy.ps1
```

并以低并发串行运行 Go、Rust、前端和扩展 fixture 测试，避免再次压满本机资源。

## 8. 界面原则

- Connect 主页面不增加“上传图片”“去重”“上下文模式”三个新按钮。
- 历史行用图标和简短文字显示 `1 张图片`、`等待页面`、`投递未知`。
- 技术状态和 context mode 放入现有“连接设置”折叠区。
- 正常用户仍只做一件事：在 Telegram 发送文字或图片。

## 9. 本阶段明确不做

- 不接入微信。
- 不同时启动飞书图片开发。
- 不用 base64 塞入 prompt 或 SQLite。
- 不修改系统网络、代理、DNS、hosts、证书或 443。
- 不通过屏幕坐标上传图片。
- 不自动外发 Claude Science 页面或工作区里的任意图片。
- 不宣称跨 Telegram、SQLite、Chrome DOM 的数学严格 exactly-once。

## 10. 下一次开工的第一个任务

完成 **P3 真人入站图片复验**：

1. 从手机 Telegram 发送一张 JPEG，无 caption。
2. 再发送一张 PNG，并附带一句分析要求。
3. 确认 Chrome 中 Claude Science 出现图片预览，图片与 caption 进入同一用户轮次。
4. 重放同一 Telegram update，确认只产生一次文件物化和一次 DOM 提交。

该复验通过后，再决定是否为飞书补齐同样的媒体能力；当前不扩展微信。

## 11. 2026-07-21 实施审计结果

本计划已经按原六点顺序推进到第六点：

1. characterization 与上下文瘦身：已完成。默认使用短包装，保留旧格式开关；固定完整 message ID 作为回传关联，不把正文写入诊断日志。
2. 有效单次投递：已完成。增加 delivery ledger、稳定浏览器 task ID、插件本地任务账本，以及 `delivery_unknown` 停止盲重试。
3. Telegram 图片安全接收：已完成。支持最大照片和图片文档、caption、JPEG/PNG/WebP magic bytes、20 MB 上限、私有目录原子落盘、SHA-256 与重复 update 去重。
4. Claude Science 页面图片投递：已完成代码。通过一次性本地 capability URL、原生 file input、Blob 校验、预览确认和图片/正文同一轮提交；预览失败不会发送 caption 残片。
5. 故障注入与自动回归：已完成。Go、Rust、Python、TypeScript、Vite、扩展语法和 Bridge 两个硬门均通过，证据见 `reports/csa-connect-message-v3/`。
6. 运行态与人工复验：WSL 与 Gateway 已恢复；TG 文本闭环和 Claude Science artifact 图片回到真实 Telegram 均已验收。Telegram 图片进入 Chrome 页面仍待真人复验，不将自动测试伪装为真人通过。

7. Claude Science 图片回到 Telegram：已完成。Bridge 提取 `{{artifact:<uuid>}}`，Gateway 只读解析 Claude Science artifact index，并在受管根目录、请求时间、MIME、magic bytes、大小和 SHA-256 全部通过后调用 Telegram `sendPhoto`。每张图片使用独立 delivery ledger；真实 Telegram 返回平台 message ID `40`，重复扫描未重复发送。

当前不启动飞书图片开发。数学意义上的 exactly-once 仍不承诺，产品行为目标是 effectively-once。P3 入站图片的代码与自动测试已完成，但仍需用户从手机发送一张新图片，复验 Chrome 页面实际预览和提交。
