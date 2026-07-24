# CSA Connect 真实计划 v2 — TG 先行(2026-07-18)

> 本文替代 v1(`CSA_connect_return_path_plan_v1_20260718.md`,已废弃)。执行者:本地 Codex,目标模式,配套 prompt `docs/prompts/CSA_connect_codex_goalmode_prompt_v2.md`。
> 决策人:吕博。每道门(G0'~G5')由吕博亲测验收后才进下一步。

---

## 0. v1 错在哪(先纠偏,防止照旧施工)

| v1 的假设 | 实测真相(2026-07-17/18 从 4080 取证) |
|---|---|
| 回程要新建一个无头 responder 替 Claude Science 回答 | **错。** Claude Science 是主角,遥控消息必须由**真身**回答;无头代答没有意义 |
| MCP 认领链路可能配不通 | **错。** Telegram 实测:3 条消息全部 received→queued→**claimed**,最快 2 秒被认领——MCP 认领已在工作 |
| 缺口在"读 inbox→产回复→写 outbox"整段 | **只对一半。** 真正缺口只有一环:**回复文本的捕获与投回**(3 条消息全部卡死 claimed,库里 0 条 outbound、0 个 response.sent) |
| 先飞书 | **改 TG 先行**(TG 配对已实测通;飞书扫码建 bot 麻烦,放第二阶段) |
| 外援用 claude -p 无头跑 | **改有头模式**(吕博要求 cc2wechat 式:可见窗口+会话留档+可插话) |

## 1. 已实测事实(计划地基,勿重复验证)

- **Gateway 正在跑**:WSL PID 见 `~/.local/share/claude-science-api-bridge/connect/gateway.pid`,DB 同目录 `connect.db`。
- **TG 通道已通**:账号 8878757803 配对成功,线程绑定工作区 `/mnt/c/Users/Admin/Documents/New project 5/claude-science-api-bridge`(库里已是 WSL 路径,路径转换无问题)。
- **回程管道已建成**:Gateway 每 1 秒扫绑定工作区 `.csa/connect/v1/outbox/`,合法回执自动投回 TG 并删文件(`ScanOutboxes`→`SendReply`);TG 半流式编辑同一条消息也已实现(`SendProgress`)。
- **外援执行内核已通**:`reports/csa-agent-runs/` 实测成功(7-17 20:36 运行,exit 0,result.json 带 `session_id` + `resumeCommand`;7-15 有 continue-* 多轮续跑记录)。
- **Bridge 是全量咽喉**:Claude Science 每次模型调用都经 WSL 的 proxy.py(127.0.0.1:9876),且已有 `/api/recent-requests` 记录机制——说明请求/响应聚合能力已存在。
- **库里有 3 条卡在 claimed 的旧消息**(aaf917a3/8f9a8dcb/95e83cd5),M1 验收时用 `Requeue` 或新消息测试。

## 2. 核心设计(定稿)

```
【遥控主线】TG → Gateway(排队) → 注入真身 Claude Science 页面(消息带标记 [CSA#短id])
            真身正常回答 → 模型响应流经 Bridge(proxy.py)
            → Bridge 旁路抄送:认出带标记的那一轮,聚合回复文本,写 outbox
            → Gateway 1 秒内扫到 → 投回 TG                    【全程 0 额外 token】

【外援支线】面板/求助单 → 弹可见终端窗口(交互式 claude,固定 --session-id)
            → 人可看可插话;机器旁听 Claude Code 会话转录(jsonl)收割结果
            → 结果落 runId 文件夹,可 --resume 继续

【中枢】  死代码(Gateway+面板),不含任何智能体,0 token
【飞轮】  research_events 表已在自动记事件,后续只加统计页,不在本计划范围
```

安全底线沿用 AGENTS.md Prime Directive:不动网络/代理/DNS/hosts,不装根证书,不绑 443,不打印密钥。**proxy.py 的抄送必须是加法且带开关**(config 里 `connect_tap_enabled`,默认关,吕博机器开),改完 `self-test.ps1` + `verify-proxy.ps1` 必须仍全绿——Bridge 是已发布组件,不许破坏主业。

## 3. TG 时间线(具体到天;每门吕博亲测后放行)

### M0 · 7-18(今天,半天)— 抄送可行性钉死,不写正式代码
1. 读 proxy.py:找到请求体里 messages 的解析点、响应文本的聚合点(/api/recent-requests 存的地方),写出"tap 挂哪两行"的说明。
2. 用面板"本地发送"注入一条带 `[CSA#test1]` 标记的消息到 Claude Science 页面,在 /api/recent-requests(或日志)确认:①标记在请求体可见;②回复全文可聚合。
**门 G0'**:两个"确认"都有截图/输出证据。任一不成立→停,报告,重新选路。

### M1 · 7-19 ~ 7-20 — TG 回程最小闭环(本计划的心脏)
1. proxy.py 加旁路抄送(加法+开关):检测最新 user 消息含 `[CSA#<messageId短码>]` → 流结束后聚合 assistant 全文 → 按 `SandboxReplyV1` 原子写 `<绑定工作区>/.csa/connect/v1/outbox/<messageId>.json`(status=replied);同一 messageId 只写一次(outbox/ack 已存在则跳过)。
2. 打通"TG→页面"投递:排队消息自动注入页面并带标记(复用现有插件注入通道;由面板轮询 Gateway 待投消息触发,注入成功记 delivered 元数据,失败保留队列重试)。
3. 端到端:TG 发"介绍一下你自己"→ 真身在页面回答 → TG 收到同一段回答。
**门 G1'**:①TG 连发 3 条,3 条都收到真身回复;②Bridge 开关关掉后一切回到原状;③self-test/verify-proxy 全绿;④旧的 3 条 claimed 消息被 requeue 后也能走完或明确标记过期。

### M2 · 7-21 — 稳定性加固
超时(如 5 分钟无回复→TG 回"真身未响应,已转入队列,可稍后再问");页面不可达(Chrome 没开)→ 消息滞留队列并 TG 提示;注入前查 composer 忙闲,忙则延后;幂等(重复事件不重复注入/不重复回);Gateway/面板重启后积压自动补投。
**门 G2'**:关掉 Chrome 发消息→得到明确提示且不丢;重开 Chrome→积压消息补走完;同一条消息不出现两次回复。

### M3 · 7-22 ~ 7-23 — 外援有头模式(cc2wechat 式)
派工时弹**可见终端窗口**(wt.exe/cmd start)跑交互式 claude,固定 `--session-id <uuid>`;旁听 `~/.claude/projects/**/<session-id>.jsonl` 转录收割 assistant 消息→写 result.json(沿用 csa-agent-runs 契约);窗口保留,人可直接续聊;面板列出历史会话夹,一键 `--resume`。
**门 G3'**:布置一个任务→自动弹窗可见执行→人插一句话它接得住→结果仍被机器收进 runId 夹。

### M4 · 7-24 — 极简 UI + 一键配对 + logo
极简三原则:**每屏一个主按钮;能自动的不解释;细节全部收进"高级"折叠**。
1. TG 一键配对:面板一个按钮生成深链 `https://t.me/<bot>?start=<配对码>`(+二维码),Gateway 把 `/start <code>` 等同 `/pair <code>` 处理——手机点一下即配对,零输入零解释。
2. 插件零配置:装好后面板检测到心跳自动完成 token 配对(或降级为"复制一次配对码"),插件 UI 只留一个绿点状态。
3. 对话框预算:全流程用户可见界面 ≤3(面板 Connect 页 / TG 聊天 / 外援窗口),多余弹窗、说明文案、二级确认一律砍掉或折叠。
4. logo 更换:替换 launcher 的 Tauri 图标资产(样式待吕博一句话定调:极简单色字母 C / 桥形图标 / 其他;资产未定前先出 2-3 个候选)。
**门 G4'**:吕博用手机从零走一遍配对≤30 秒、无需看任何说明;界面数不超预算;新 logo 上机。

### M5 · 7-25 ~ 7-27 — 飞书接入(第二信道)
复用同一条管线(架构信道无关,TG 已验证),只做飞书通道适配:扫码一键建 bot(代码已有)→配对→绑定→同一套注入+抄送回程。
**门 G5'**:飞书私聊完成一问一答;TG 同时不受影响。

> 时间为目标节奏(Codex 每日一场施工+吕博当日验门);哪天门没过就停在哪,不许带伤推进。

## 4. 明确不做(本计划范围外)
- 不做群聊、多用户、公网端口、云中继。
- 不做飞轮统计页(原料已在 research_events 自动积累,另立计划)。
- 不重构 Subagent Hub 旧队列合并(等 M3 外援稳定后另议)。
- 不动 API Kit。

## 5. 交付与证据
每个 M 完成:改动文件清单 + 证据(命令输出/TG 截图,脱敏)落 `reports/csa-connect-return/M<N>/`,并在门报告里逐项勾选。
