# MiniMax 中国区与 DeepSeek Provider 本地构建记录

日期：2026-07-11

版本：CSA v0.1.3 本地候选版

状态：静态、单元、包级与真实 Key 基础验收通过
发布：未上传 GitHub

## 已实现

- MiniMax 官方模板切换为中国区 `https://api.minimaxi.com/anthropic`。
- 官方文档确认 `MiniMax-M3` 及支持的 M2.x Anthropic 模型 ID。
- MiniMax 初始模型保持为空，不把 M3 写成不可修改的默认值。
- 修复 Rust Provider 目录中的 MiniMax 中文 badge 乱码。
- DeepSeek 保留当前官方 `deepseek-v4-pro` / `deepseek-v4-flash` 与兼容别名原意。
- `Deep-chat` 仅修正为 `deepseek-chat`；`glm-5.2` 等无关模型不再静默变为 DeepSeek Pro。
- API Key 测试不再混入跨 Provider 固定候选清单；优先使用手动模型或实时列表。DeepSeek 在列表不可用时只尝试经过资料核验的官方候选，并要求真实短对话成功后才保存。
- 原生 Anthropic 兼容请求会把 Claude Science 的 `thinking.type=auto` 转换为 `adaptive`，避免 DeepSeek/MiniMax 因不识别 `auto` 返回 400。
- Bridge 默认 Provider 改为空状态。

## 验证结果

| 验证 | 结果 |
| --- | --- |
| Rust 单元测试 | 37 passed |
| Bridge 检测回归 | 9 passed |
| Python translation tests | 51 passed |
| `scripts/self-test.ps1` | passed |
| TypeScript + Vite production build | passed |
| Release build | passed（`CARGO_BUILD_JOBS=1`） |
| 便携目录自测 | passed |
| EXE 启动冒烟 | 启动 6 秒后存活且 Responding |
| ZIP SHA256 | match |
| EXE / vendor manifest hash | match |
| ZIP 运行时完整性 | EXE、Skill、Claude Science binary、manifest 均存在 |
| 私有目录 | 不存在 |
| `sk-...` 敏感模式 | 未发现 |

## 真实 Provider 验收

| Provider | 验收结果 |
| --- | --- |
| DeepSeek 官方 | Key 有效；`deepseek-v4-pro` 非流式 HTTP 200、流式 HTTP 200，最近请求均为 `success` |
| DeepSeek thinking | Claude Science 的 `auto` 转换为 `adaptive` 后通过，原失败不是 Key 问题 |
| MiniMax 中国区 | 基础连通确认可用；输出上限、工具调用和 reasoning 完整能力仍需显式能力探测 |
| MiniMax 国际区旧域名/乱码 | 未发现 |

第一次并行 release 编译时 `rustc` 在 Windows 返回 `0xc0000005`。保持源码不变、将 `CARGO_BUILD_JOBS=1` 后成功，因此记录为本机构建工具链的瞬态/并发稳定性问题，不伪装为代码成功。后续 CI 建议固定单并发或更换已验证 Rust 工具链后再评估。

## 产物

- `dist/candidate-minimax-cn-deepseek-20260711/claude-science-assistant-v0.1.3-release-portable.zip`
- `dist/candidate-minimax-cn-deepseek-20260711/claude-science-assistant-v0.1.3-release-portable.zip.sha256`

## 未完成的真实验收

本轮没有接收、读取或调用用户的 MiniMax/DeepSeek API Key，因此不能声称 Provider 已完成线上连通。下一步由用户在候选版启动器内输入中国区 MiniMax Key，手动填写或读取模型 `MiniMax-M3`，依次完成：

1. 启动器测试对话。
2. 保存并切换活动 Key。
3. 自动映射（模型列表可用时）。
4. Bridge `/v1/messages` 请求。
5. Claude Science 内实际回答与工具调用。

所有结果只保存脱敏状态、HTTP 状态、模型 ID、配置 revision 与请求 ID，不保存 Key 或回答正文。
