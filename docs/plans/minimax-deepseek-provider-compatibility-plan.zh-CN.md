# CSA MiniMax M3 与 DeepSeek 适配计划

状态：`LOCAL BUILD + BASIC LIVE VERIFICATION COMPLETE`，官方契约、本地构建与基础真实 Key 验收已完成

日期：2026-07-11
范围：启动器 Provider 模板、模型发现、直连测试、自动映射、Bridge 全链路与升级兼容

## 1. 目标

让用户在“添加 API Key”中选择 MiniMax 或 DeepSeek，输入自己的 Key 后完成以下闭环：

1. 优先读取服务商真实模型列表；若 DeepSeek `/models` 不返回可测试列表，可尝试经过资料核验的官方候选，但只有真实短对话成功后才允许保存。MiniMax 仍可由用户填写官方模型 ID 后测试。
2. 在启动器测试对话框内直接发起一次短对话，不需要先启动 Claude Science。
3. 根据真实返回模型生成主力/快速映射；只有一个模型时统一映射，不制造不存在的快速模型。
4. 保存并切换 Key 后，Bridge 使用同一 Provider、Base URL、协议和模型映射。
5. 通过 Bridge 与 Claude Science 各完成一次真实回答，证明“测试能通”和“实际使用能通”是同一条配置链。

MiniMax 中国区官方文档已确认 `https://api.minimaxi.com/anthropic` 与模型 ID `MiniMax-M3`。CSA 仍不把它写成不可更改的默认值，而是由账户模型列表或用户明确输入后测试。DeepSeek 官方文档已确认当前 `deepseek-v4-pro` / `deepseek-v4-flash`；初始状态仍为空，候选必须通过真实对话后才写入。

## 2. 当前代码事实

当前工程已经存在部分适配骨架：

- Provider 列表已有 `minimax` 与 `deepseek`。
- MiniMax 模板已改为中国区 `https://api.minimaxi.com/anthropic` 和 Anthropic-compatible 模式。
- DeepSeek 当前模板使用 `https://api.deepseek.com/anthropic` 和 Anthropic-compatible 模式。
- 启动器已有 `/models` 探测、短对话、自动映射与 DPAPI 加密保存 Key 的入口。
- Bridge 已支持 Anthropic 透传和 OpenAI-compatible 协议转换。

但“模板存在”不等于“适配完成”。当前仍有以下待核实项：

- Rust Provider 目录中 MiniMax 的中文 badge 存在乱码，必须修正并增加回归测试。
- `MiniMax-M3`、`deepseek-v4-pro`、`deepseek-v4-flash` 已由官方文档确认；它们只能用于识别实时列表，不能作为空状态默认值。
- DeepSeek 兼容函数不再把旧名或 `glm-5.2` 静默归一到 Pro；只修复已知语音输入拼写 `Deep-chat`，其他值原样交给测试暴露错误。
- `/models` 不可用、返回空列表、鉴权方式不同或区域域名不同的行为尚未通过真实 Key 验收。
- 直连测试成功后，尚需证明保存、切换、Bridge 配置写入和 Claude Science 请求保持一致。

## 3. 安全边界

- 不把 API Key 写进聊天、Prompt、Markdown、测试代码、截图、日志或 Git。
- 推荐由用户在 CSA 启动器的 Key 输入框中填写；启动器只以 Windows DPAPI 密文持久化。
- 诊断结果只保留 Provider、经过校验的 Base URL、HTTP 状态、错误类别、模型 ID 与脱敏请求 ID。
- 若 Key 曾经发送到聊天或其他明文位置，应视为已暴露并在测试后轮换。
- 每次真实对话测试前显示将访问的官方域名、协议、模型和最大输出预算，用户确认后才发送。
- 不为了“自动适配”把同一个 Key 轮询发送到多个候选域名。

## 4. 第一阶段：官方契约核验

仅采用 MiniMax、DeepSeek 官方文档与真实接口响应，建立版本化能力表：

| 项目 | MiniMax | DeepSeek |
| --- | --- | --- |
| 官方控制台/文档域名 | 待官方核验 | 待官方核验 |
| 中国/国际 Base URL | 分开记录，不自动跨域尝试 | 按官方文档记录 |
| 协议 | Anthropic-compatible / OpenAI-compatible 实测 | Anthropic-compatible / OpenAI-compatible 实测 |
| 鉴权头 | 待官方核验 | 待官方核验 |
| 模型列表 | 路径、权限、分页、空列表行为 | 路径、权限、分页、空列表行为 |
| 对话接口 | 路径、请求体、响应正文位置 | 路径、请求体、响应正文位置 |
| 模型 ID | 以接口真实返回为准 | 以接口真实返回为准 |
| 工具调用/流式 | 支持状态与限制 | 支持状态与限制 |
| 输出参数 | 使用官方默认或模型能力，不全局钉死 | 使用官方默认或模型能力，不全局钉死 |

输出为代码内 Provider capability，而不是散落的 `if provider == ...` 字符串。模型名和协议能力需要带来源与验证日期。

## 5. 第二阶段：实现调整

### 5.1 Provider 模板

- MiniMax 与 DeepSeek 均保持初始模型为空。
- Base URL 必须显示并锁定到所选官方区域；区域切换是显式选择，不是失败后自动换域名。
- 修正 MiniMax 中文文案乱码。
- Provider 配置记录 `protocolMode`、`modelsEndpointMode`、`authMode`、`region` 与 `verifiedAt`。

### 5.2 模型发现与手动回退

- `/models` 成功：只在真实返回列表中评分并生成映射。
- `/models` 不支持或无权限：显示准确原因，允许用户从官方控制台复制模型 ID；不凭内置猜测发送候选请求。
- 用户填写的模型必须先做一次短对话验证，再允许保存为活动模型。
- 一个模型：Sonnet/Opus/Haiku 全部映射到同一个模型，并明确显示“该服务当前只有一个已验证模型”。
- 多个模型：主力优先能力更高且非 high-speed/flash/turbo 的模型；快速优先官方标记的高速模型。评分只排序真实列表，不创造模型。

### 5.3 输出参数适配

- Bridge 不全局写死一个很小的 `max_tokens`。
- 启动器连通测试可以使用较小预算，但若只返回 reasoning 或 `stop_reason=max_tokens`，只对测试请求做一次受限重试。
- 正式 Claude Science 请求优先保留上游请求值；只在官方文档明确模型上限时做上限裁剪。
- Provider 不支持的 `temperature`、`top_p`、thinking 或工具字段应按能力表剔除/转换，并在测试中覆盖。

### 5.4 配置切换一致性

- Key 列表选择项是唯一活动配置入口。
- 保存、切换和 Bridge 重启必须绑定同一个 `apiKeyId + providerId + baseUrl + modelAliases + revision`。
- Bridge 健康检查除端口外，还要返回脱敏后的 Provider ID、模型映射摘要和 revision。
- UI 只有在 revision 与活动 Key 一致时才显示“已切换”；否则显示“配置未生效”，不能只凭保存成功。

## 6. 第三阶段：测试顺序

每个 Provider 都按相同顺序执行，不直接从“Key 有效”跳到“已适配”：

1. **静态/单元测试**：URL 规范化、鉴权头、响应解析、模型评分、空列表、超时、401/403/404/429、乱码和密钥脱敏。
2. **Mock 契约测试**：模拟 Anthropic 与 OpenAI 两种协议、文本块、reasoning-only、工具调用、流式与模型列表分页。
3. **启动器直连测试**：用户在 UI 输入 Key，先读取模型，再在对话框提问“请只回答：连接成功”。记录脱敏证据。
4. **保存与切换测试**：保存 Key、切到另一 Key、再切回，确认 revision 和 Provider 摘要变化。
5. **Bridge 测试**：通过本机 9876 发送一次 Anthropic messages 请求，确认上游模型与返回正文。
6. **Claude Science 验收**：在应用内发起普通回答和一次工具调用，确认不是只通了测试脚本。
7. **升级回归**：旧版本保存的 DeepSeek/MiniMax 配置升级后可读取；不存在的旧模型名只提示重新选择，不静默映射到另一个收费模型。

MiniMax 与 DeepSeek 分别生成一份脱敏验收记录；一个通过不能代替另一个。

## 7. 真实 Key 协作方式

推荐流程：

1. 我先完成官方契约核验、Mock 测试与代码修正。
2. 构建本地候选版，不上传 GitHub。
3. 你在启动器中自行填入 MiniMax Key，选择官方区域与模型，运行“测试连通”。
4. 你把脱敏后的结果、HTTP 状态、请求 ID 和界面现象交给我；不要粘贴 Key。
5. MiniMax 修正并通过全链路后，对 DeepSeek 重复同样流程。
6. 两者都通过后再决定是否打包发布。

若必须由自动化运行真实测试，应通过当前用户会话的安全输入或临时环境变量注入，进程结束立即清除；不把 Key 作为命令行参数。

## 8. 验收标准

- 初始模型为空，没有钉死 MiniMax M3、DeepSeek Pro/Fast 或其他默认模型。
- 模型列表存在时，只映射列表真实返回的模型。
- 模型列表不存在时，手动模型可验证且错误信息可操作。
- 启动器测试对话、Bridge 和 Claude Science 三层都返回有效正文。
- 切换 Key 后 revision、Provider、Base URL 和模型映射同步变化。
- 401、403、404、429、超时、空正文和 reasoning-only 能被区分。
- Key 不出现在日志、错误、计划、测试快照或 Git diff 中。
- UI 不冻结；网络操作在后台运行且可超时恢复。
- Windows 10/11、Ubuntu-22.04/24.04 的 Bridge 配置行为一致。

## 9. Build Gate

在开始真实 Key 测试前需要确认：

- MiniMax 使用中国区还是国际区官方账户及对应 Base URL。
- 用户在启动器内输入 Key，不在聊天中发送。
- 真实测试允许产生极少量 API 费用。
- 先在本地候选包测试，不上传 GitHub。

当前可先做官方文档核验、Mock 测试与代码审查；真实 Key 阶段等待用户提供账户区域并在 UI 内输入。
