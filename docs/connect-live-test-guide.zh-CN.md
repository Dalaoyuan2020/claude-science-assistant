# CSA Connect 飞书与 Telegram 实机测试

## 测试目标

2026-07-20 的已验收基线是 Telegram。执行新一轮回归时先完成第三节；飞书章节保留为后续通道测试，不作为当前 TG-first 基线的前置条件。

按顺序验证四段链路，任何一段失败都不要跳到后面：

```text
Telegram -> Connect Gateway -> 浏览器插件 -> Claude Science
Claude Science -> Bridge tap -> Connect Gateway -> 原聊天消息分段更新
```

## 一、共同准备

1. 启动 CSA 桌面客户端，进入 `Connect`。
2. 安装并配对 `CSA Claude Science Page Connector` 浏览器插件。
3. 在 Chrome 打开 Claude Science，并确认插件状态为 `页面就绪`。
4. 当前工作区路径必须填写正确；它用于把外部聊天线程绑定到当前科研项目。
5. `安装 Skill` 和 MCP Connector 仅用于降级测试。正常自动闭环不要求模型主动调用回复工具，也不要把 Token 发到聊天或截图中。

## 二、飞书后续通道测试

### 2.1 创建机器人

推荐路径：

1. 在 `消息通道 -> 飞书` 点击 `配置`。
2. 点击 `扫码创建飞书机器人`。
3. CSA 会打开飞书官方的一次性创建页面。使用飞书登录或扫码确认。
4. 页面确认后等待数秒，CSA 应显示飞书 `等待配对`，Gateway 显示 `后台运行`。

该流程使用飞书官方“一键创建智能体应用”能力，自动创建企业自建应用、机器人、消息权限和长连接事件。链接 10 分钟有效，App Secret 返回后由 CSA 使用 Windows DPAPI 加密保存。

备用路径：已有企业自建应用时，手动填写 App ID/App Secret。应用至少需要：

- 机器人能力。
- `im:message.p2p_msg:readonly`。
- `im:message:send_as_bot`。
- `im:message:update`，用于半流式编辑同一条回复。
- 事件 `im.message.receive_v1`。
- 事件接收方式为长连接。

### 2.2 配对和项目绑定

1. 在 CSA 飞书通道点击 `配对`。
2. 在飞书中找到刚创建的机器人，私聊发送面板给出的 `/pair <code>`。
3. 收到“配对成功”后，再发送：`连接测试 01`。
4. 回到 CSA，`项目路由`应出现一条飞书线程。
5. 填写当前 Claude Science 工作区路径，点击 `绑定`。

### 2.3 消息触发测试


在飞书私聊发送：

```text
请只回复：CSA-FEISHU-OK，并补充一句当前会话正在处理的项目名称。
```

通过标准：

1. CSA 队列短暂从 `排队` 变为 `处理中`。
2. Chrome 当前 Claude Science 输入框自动收到一条带 `[CSA#...]` 关联标记的请求并提交。
3. 投递不需要点击 Claude Science 的发送按钮。
4. 飞书最终收到回复，内容包含 `CSA-FEISHU-OK`。

### 2.4 半流式测试

发送：

```text
请用三段回答：第一段说明收到消息，第二段列出两个检查项，第三段给出结论。每段完成后更新回复。
```

通过标准：飞书只出现一条主要回复，该消息内容至少更新两次，最后状态变为完成；不应连续刷出三条独立消息。

## 三、Telegram 测试

1. 在 Telegram 私聊 `@BotFather`。
2. 发送 `/newbot`，按提示设置名称和用户名，取得 Bot Token。
3. 在 CSA `消息通道 -> Telegram -> 配置`中填入 Token，点击 `保存并启动`。
4. 打开新机器人私聊并发送 `/start`。
5. 在 CSA 点击 `配对`，向机器人发送 `/pair <code>`。
6. 再发送一条普通消息，回 CSA 绑定工作区。
7. 重复飞书的消息触发测试和三段半流式测试。

Telegram 使用 `getUpdates` 长轮询，不需要公网端口或域名。如果该 Token 以前配置过 Webhook，需要先调用 Bot API `deleteWebhook`，否则 `getUpdates` 无法同时使用。

### 3.1 Claude Science 图片回到 Telegram

在 Telegram 发送：

```text
请生成一张简单的科研示意图，并把最终图片返回给我。
```

通过标准：

1. Telegram 的文字占位消息被更新为最终正文。
2. 随后收到一条真实图片消息，不显示 `{{artifact:...}}` 内部标记。
3. 同一回复不会重复发送第二张相同图片。
4. Gateway delivery ledger 中对应 `telegram_artifact:<version-id>` 为 `submitted`。

### 3.2 Telegram 图片进入 Claude Science

1. 发送一张 JPEG，不带 caption。
2. 再发送一张 PNG，caption 写“请概括图中内容”。
3. Claude Science 页面必须先出现图片预览，再提交用户轮次。
4. 第二条消息的图片和 caption 必须进入同一用户轮次；预览失败时不得只发送 caption。

当前该方向已通过自动测试，仍需在真实 Chrome 页面完成一次人工复验。

## 四、故障定位

| 现象 | 优先检查 |
|---|---|
| 飞书扫码页未打开 | 网络能否访问 `open.feishu.cn`，链接是否超过 10 分钟 |
| 机器人收不到私聊 | 应用是否发布/可用，是否开启机器人、单聊读取权限和 `im.message.receive_v1` |
| CSA 收到但 Claude Science 未触发 | 浏览器插件是否 `页面就绪`，Chrome 是否打开正确的 Claude Science 页 |
| Claude Science 已回答但聊天端没回复 | Bridge tap 是否已开启、workspace ack 是否为 `claimed`、outbox 是否被 Gateway 消费 |
| 同一消息被重复投递 | 不要同时运行两个 CSA 桌面实例；检查 Gateway 是否只有一个进程 |
| 半流式变成多条消息 | 检查 Bridge progress sequence 是否递增、Telegram progress message ID 是否被复用、最后是否 `final=true` |
| 回复正文有 `{{artifact:...}}` 但没有图片 | 检查 Gateway 是否为最新二进制、artifact 是否属于当前请求、文件校验与 `telegram_artifact` 账本状态 |

## 五、验收记录

每次测试只记录以下非敏感信息：时间、通道、测试编号、队列状态变化、是否自动触发、是否收到最终回复、是否分段更新、错误摘要。不要记录 App Secret、Bot Token、MCP Bearer Token 或完整私聊正文。
