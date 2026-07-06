# CSA — Claude Science Assistant

> 绿皮书第二章配套工具：在 Windows 上把 Claude Science 稳定启动起来，并接入 GLM、LongCat、DeepSeek、MiniMax、OpenCode Go、OpenRouter 以及自定义 OpenAI-compatible 中转。

CSA 的目标很朴素：让普通 Windows 用户不用反复折腾 WSL、端口、模型名和 API Key，也能把 Claude Science 跑起来。它不是一个新的大模型平台，也不是破解工具；它是一个本地启动器、运行时编排器和 API Bridge 管理面板。

当前版本：`v0.1.1`

## 为什么做 CSA

Claude Science/Claude Code 这类科研工作流真正难的地方，往往不是“会不会写 Prompt”，而是第一步就被环境拦住：

- Windows 用户要处理 WSL2、Ubuntu、Linux 运行时和 localhost 转发。
- 国内模型、聚合平台、第三方中转的接口协议与模型名不完全一致。
- Claude 侧习惯使用 `sonnet`、`opus`、`haiku` 等角色名，但上游 Provider 往往使用自己的模型 ID。
- API Key 需要保存、切换、测试，又不能暴露在日志、截图和发布包里。

CSA 把这些问题收口成一套产品逻辑：

1. 先体检电脑，确认 Windows/WSL/端口/运行时状态。
2. 再由启动器管理唯一的 WSL 运行时。
3. API Key 在“添加 API Key”入口中按模板添加，而不是把所有 Key 平铺在首页。
4. 启动前测试连通与自动映射模型。
5. 最后把 Claude Science 指向本地 Bridge，由 Bridge 负责协议转换、模型映射和上游调用。

这也是本项目和 [Claude Science 绿皮书](https://github.com/Dalaoyuan2020/claude-science-green-book) 的关系：绿皮书讲“怎么把 Claude Science 用到科研流程里”，CSA 负责让读者先把工具装上、跑通、接入模型。

## 实现原理

CSA 采用“Windows 启动器 + WSL 运行时 + 本地 Bridge + 上游 Provider”的分层架构。

```mermaid
flowchart LR
    U["用户"] --> L["Windows 启动器\nTauri + React"]
    L -->|"体检 / 安装 / 启动 / 停止"| W["WSL2 Ubuntu\n推荐 Ubuntu-24.04"]
    W --> CS["Claude Science\nLinux 运行时"]
    CS -->|"Anthropic Messages\n127.0.0.1:9876"| B["CSA Bridge\nFastAPI"]
    B -->|"Anthropic 透传\n或 OpenAI Chat Completions 转换"| P["官方 API / 聚合平台 / 第三方中转"]
    L --> K["API Key 管理\nWindows DPAPI 加密"]
    K -->|"激活后写入 WSL 运行时配置\n0600 权限"| B
```

核心思想是：Windows 负责用户体验，WSL 负责运行 Claude Science 和 Bridge，Bridge 负责把 Claude Science 发出的 Anthropic-style 请求转换到上游模型。

| 层 | 做什么 | 为什么这样设计 |
| --- | --- | --- |
| Windows 启动器 | 展示状态、添加 API Key、测试连通、启动/停止服务 | 普通用户双击即可使用，不需要手动进终端 |
| WSL 运行时 | 承载 Claude Science、Bridge、Python 依赖和运行日志 | 避免 Windows/WSL 双 Bridge 分叉，保持一个真实运行源 |
| CSA Bridge | 协议转换、模型映射、Provider 调用、健康检查 | 让 Claude Science 以熟悉的 Anthropic 接口工作 |
| Provider 模板 | 官方直连、聚合平台、第三方中转、自定义 Base URL | 降低模型接入门槛，同时保留高级用户自由度 |
| 体检 Skill | 只读检查、安装计划、修复、回滚 | 新电脑先诊断再改系统，减少“越修越乱” |

## 从 CC-switch 借鉴什么

CSA 的交互会参考 [CC-switch](https://github.com/farion1231/cc-switch) 这类轻量配置切换工具，但不是照搬：

- 借鉴“配置是一组有顺序的条目”的思路：API Key 按添加顺序排列，当前只激活一条。
- 借鉴“添加入口集中管理”的思路：官方 API、聚合平台、中转、自定义都在“添加 API Key”里选择。
- 借鉴“先测试再启用”的思路：保存前可测试连通，必要时自动映射模型。
- 不把所有 Provider 平铺到首页。首页应该只显示当前状态、当前 Key、启动按钮和必要诊断。
- 不直接 fork CC-switch。CSA 的核心难点是 WSL 生命周期、Claude Science 运行时和协议 Bridge，不是单纯切换 CLI 配置。

换句话说，CC-switch 给我们的是产品节奏：入口清楚、配置有序、切换可控；CSA 自己要补上 WSL、Bridge、模型映射和安全边界。

## 与 Claude Science 绿皮书联动

CSA 是绿皮书“上手篇 §02 装上你的科研搭档”的 Windows 落地层。

| 读者状态 | 推荐入口 |
| --- | --- |
| 还不知道 Claude Science 能做什么 | 先读 [Claude Science 绿皮书](https://github.com/Dalaoyuan2020/claude-science-green-book) |
| Windows 用户，想先跑起来 | 下载 CSA Release，按本仓库教程安装 |
| 已经能启动，但不会接国产模型 | 看 CSA 的 API Key 与自动映射说明 |
| 已经跑通，想提高科研使用水平 | 回到绿皮书继续读 §03 之后的科研流程 |
| 想排查环境问题 | 使用 `bootstrap-claude-science-wsl` 体检 Skill 和 CSA 证据收集脚本 |

建议在绿皮书第二章中把 CSA 作为“Windows 推荐路径”：

```text
如果你是 Windows 用户，建议优先使用 CSA（Claude Science Assistant）。
CSA 会先检查 WSL2、Ubuntu、端口、运行时和 API Key 状态，再帮你启动 Claude Science。
这一步的目标不是学习系统运维，而是尽快进入科研工作流。
```

更完整的联动文案见 [docs/green-book-integration.zh-CN.md](docs/green-book-integration.zh-CN.md)。

## v0.1.1 做到了什么

- Windows 便携启动器：双击 `claude-science-assistant.exe` 使用。
- WSL 单运行时路线：推荐 Ubuntu-24.04，同时兼容已安装的 Ubuntu/WSL 发行版。
- API Key 列表：首页按添加顺序显示已保存 Key，当前只激活一条。
- “添加 API Key”模板入口：官方、聚合、中转、自定义在同一个入口选择。
- API Key 加密保存：使用 Windows 当前用户 DPAPI；界面、日志、发布包不回显明文 Key。
- 测试连通：在启动器里直接测试 API Key，不必跳到 Claude Science 项目里试错。
- 自动模型映射：读取 `/models` 后自动生成主力/快速模型映射。
- DeepSeek、LongCat、MiniMax、OpenCode Go 兼容修正。
- BAT 与 PowerShell 脚本并存：新手可双击 BAT，熟悉命令行的用户可继续用 PS1。

## Provider 默认顺序

添加 API Key 时，模板按这个顺序出现：

| 分组 | 模板 | 默认/推荐模型 |
| --- | --- | --- |
| 官方直连 | GLM-5.2 | `glm-5.2` |
| 官方直连 | LongCat | `LongCat-2.0` |
| 官方直连 | DeepSeek | `deepseek-v4-pro`，快速映射 `deepseek-v4-flash` |
| 官方直连 | MiniMax | `MiniMax-M3`，快速映射 `MiniMax-M2.7-highspeed` |
| 官方直连 | Claude | 官方登录/官方能力入口 |
| 官方直连 | OpenAI / GPT | OpenAI-compatible 接入 |
| 聚合平台 | OpenCode Go | 默认 `glm-5.2`；主力候选 `glm-5.2` → `qwen3.7-max` → `deepseek-v4-pro`；快速 `deepseek-v4-flash` |
| 聚合平台 | OpenRouter | 需要测试或手动选择可用模型 |
| 第三方中转 | 内置中转 | `https://10521052.xyz/v1` |
| 第三方中转 | 自定义中转 | 用户自行填写 Base URL |

第三方中转不会默认信任。CSA 会要求用户确认域名后，才会把 API Key 发往该地址。

## 自动模型映射

Claude Science 侧通常会请求类似 `claude-sonnet-*`、`claude-opus-*`、`claude-haiku-*` 的模型名；国产模型或中转平台则使用自己的模型 ID。CSA 的自动映射用于把两边对齐。

规则简化为三句话：

1. 如果上游只返回一个可用聊天模型，就把所有 Claude 角色都映射到这个模型。
2. 如果上游返回多个模型，优先把 Pro、Max、大模型作为主力，把 Fast、Flash、Highspeed、Mini、Lite、Air 作为快速模型。
3. 如果 Provider 有已知最佳实践，就使用 Provider 专属优先级，例如 OpenCode Go 优先 `glm-5.2`，DeepSeek 快速角色优先 `deepseek-v4-flash`。

常见映射示例：

| Claude 侧角色 | CSA 映射意图 |
| --- | --- |
| `claude-sonnet-*` | 主力模型 |
| `claude-opus-*` | 主力模型 |
| `byok-model-0001` | 主力模型 |
| `claude-haiku-*` | 快速/低延迟模型 |

这不是要让用户手填一堆一一对应关系，而是让启动器先根据模型列表生成草案；用户确认后再保存。

## 快速开始

### 1. 下载

只从 GitHub Releases 下载官方包：

- `claude-science-assistant-v0.1.1-release-portable.zip`
- `claude-science-assistant-v0.1.1-release-portable.zip.sha256`

不要从群文件、网盘或第三方镜像下载带 `claude-science-assistant.exe` 的压缩包。

### 2. 解压

解压到一个固定目录。中文路径可以使用，但如果遇到权限或杀软误报，优先换到短路径，例如：

```text
C:\CSA
```

不要只复制 exe。便携包里的 `scripts/`、`docs/`、`skills/`、`vendor/`、`static/` 等目录需要和 exe 放在一起。

### 3. 推荐方式：交给 Codex Agent 安装

普通用户不需要自己理解 WSL、PowerShell 参数和脚本路径。推荐把解压后的 CSA 文件夹作为 Codex/Codex IDE 的工作区打开，然后把下面这段 Prompt 发给 Codex：

```text
请你帮我在这台 Windows 电脑上安装并启动 CSA（Claude Science Assistant）。

请先阅读当前文件夹里的 README.md、docs/quick-start.zh-CN.md、docs/v0.1-clean-pc-acceptance.zh-CN.md，以及 skills/bootstrap-claude-science-wsl/SKILL.md。

要求：
1. 先只读体检和预览，不要安装、删除、重启或修改系统。
2. 先运行 1-run-acceptance-preview.bat，或使用 bootstrap-claude-science-wsl Skill 做只读检查。
3. 判断这台电脑是否已有可用 WSL/Ubuntu。
4. 如果已有 WSL/Ubuntu，请向我说明将要安装/修复的 CSA WSL 运行时，然后等我确认后再执行 4-install-runtime-after-preview.bat。
5. 如果没有 WSL/Ubuntu，请不要静默安装。先告诉我需要管理员权限、可能启用的 Windows 功能、推荐 Ubuntu 版本和是否可能重启；只有我明确同意后，才可以使用 -InstallWslIfMissing。
6. 不要修改 Clash、VPN、DNS、hosts、系统代理、根证书或 443 端口。
7. 不要输出、保存、截图或提交我的 API Key、OAuth token、Cookie 或控制 token。
8. 安装或修复完成后，打开 claude-science-assistant.exe，指导我添加 API Key、测试连通和自动映射模型。
9. 如果失败，请生成脱敏诊断摘要，告诉我卡在哪一步，以及下一步需要我确认什么。
```

这段 Prompt 的核心是：让 Codex 先读文档、先体检、先预览；只有用户确认后才安装或修复。

### 4. 新电脑先预览安装计划

双击：

```text
1-run-acceptance-preview.bat
```

它只预览计划，不应直接安装或修改系统。

### 5. 根据电脑状态选择安装方式

这里要分清楚三件事：

| 类型 | 是否由双击流程完成 | 说明 |
| --- | --- | --- |
| 打开 CSA 启动器 | 是 | `claude-science-assistant.exe` 是便携程序，不需要 MSI 安装 |
| 安装/修复 CSA WSL 运行时 | 是，前提是电脑已有可用 WSL/Ubuntu | `4-install-runtime-after-preview.bat` 会安装/修复 Bridge、Python venv、内置 Claude Science Linux 运行时，并启动/自测 |
| 首次安装 WSL/Ubuntu | 不默认自动执行 | 需要管理员权限、额外确认 `-InstallWslIfMissing`，并可能要求重启；也可以交给 Codex 按 Skill 引导执行 |

如果预览显示电脑已经有可用 WSL/Ubuntu，确认后再双击：

```text
4-install-runtime-after-preview.bat
```

如果预览提示没有 WSL/Ubuntu，不要把 `4-install-runtime-after-preview.bat` 当成静默系统安装器。此时有两种推荐方式：

1. 让 Codex 使用包内 `bootstrap-claude-science-wsl` Skill 继续处理，它会先解释需要启用的 Windows 功能、Ubuntu 版本、管理员权限和重启点。
2. 熟悉命令行的用户可在管理员 PowerShell 中显式运行带 `-InstallWslIfMissing` 的命令。

系统要求重启时，先重启；重启后重新运行体检/预览，再继续安装 CSA 运行时。

### 6. 启动 CSA

双击：

```text
claude-science-assistant.exe
```

首页应该显示 WSL、Bridge、Claude Science 和 Provider 状态。

### 7. 添加 API Key

点击“添加 API Key”：

1. 选择 Provider 模板。
2. 填入 API Key。
3. 对第三方中转确认 Base URL。
4. 点击“测试 API Key”。
5. 如 Provider 有多个模型，点击“自动映射”。
6. 保存并设为当前使用。

保存后点击“启动”或“打开 Claude Science”。

详细教程见 [docs/quick-start.zh-CN.md](docs/quick-start.zh-CN.md)。

## 联系与答疑

如果你在安装、API Key 接入、模型映射或 Claude Science 科研工作流里遇到问题，可以通过下面两个入口联系：

| 入口 | 用途 |
| --- | --- |
| 个人微信 | 一对一沟通、安装协助、付费支持与后续使用指导 |
| Claude Science 绿皮书答疑群 | 公开答疑、共性问题讨论、绿皮书与 CSA 使用交流 |

| 个人微信 | 答疑群 |
| --- | --- |
| <img src="docs/assets/contact/wechat-personal.png" alt="个人微信二维码" width="260"> | <img src="docs/assets/contact/wechat-group.jpg" alt="Claude Science 绿皮书答疑群二维码" width="260"> |

> 群二维码可能会过期；如果群二维码失效，请先添加个人微信获取新的入群方式。

## 安全与隐私边界

CSA 的默认策略是尽量少动系统、少暴露秘密：

- 不修改 Clash、VPN、DNS、hosts、系统代理、根证书或 443 端口。
- API Key 使用 Windows 当前用户 DPAPI 加密保存。
- DPAPI 绑定当前 Windows 用户和当前电脑；复制便携包到另一台电脑不会带走 Key。
- 当前激活的 Key 会写入 WSL 运行时配置，供 Bridge 调用上游模型使用；该文件应保持 `0600` 权限。
- 日志、验收包、文档、README、Release 说明不应包含明文 API Key。
- 分享问题截图或证据包前，仍建议人工检查一次。

如果你要提交 issue，请不要粘贴真实 API Key、OAuth token、完整 Cookie、完整日志或包含隐私内容的 Prompt。

## 常见问题

### 为什么不是纯 Windows 运行？

当前真实主链路是 Claude Science 在 WSL 中运行。把 Bridge、运行时和日志都收口到 WSL，可以避免 Windows 与 WSL 同时跑两份 Bridge，造成“面板显示成功但实际请求没切换”的问题。

### 为什么推荐 Ubuntu-24.04，但又说兼容其他版本？

Ubuntu-24.04 是默认推荐测试路径，便于复现问题；但启动器不应因为用户已有其他 Ubuntu/WSL 发行版就直接阻断。v0.1.1 的策略是“推荐 24.04，兼容已安装可用发行版”。

### 内置中转是不是官方服务？

不是。`https://10521052.xyz/v1` 是内置模板，方便测试和临时接入，但属于第三方中转。CSA 会把它归在“第三方中转”里，并要求用户确认域名。

### Claude / OpenAI 的订阅能不能当 API Key 用？

不能默认这样理解。订阅权益、网页登录和 API Key 是不同入口。CSA 只在对应 Provider 模板里处理它能实际调用的接口能力。

### API Key 列表为什么不把所有 Provider 都放首页？

首页只应该显示“当前正在使用什么”和“现在能不能启动”。所有新增 Provider、测试、自动映射都放在“添加 API Key”对话框里，避免把用户推到一堆并列配置面板前。

### 出问题先做什么？

先不要反复双击 exe。优先做三件事：

1. 在 CSA 里刷新状态。
2. 运行证据收集 BAT，生成脱敏报告。
3. 让 Codex 使用体检 Skill 重新检查环境。

排错指南见 [docs/troubleshooting.md](docs/troubleshooting.md)。

## 文档导航

| 文档 | 用途 |
| --- | --- |
| [docs/quick-start.zh-CN.md](docs/quick-start.zh-CN.md) | 新手完整接入流程 |
| [docs/architecture-and-product-plan.zh-CN.md](docs/architecture-and-product-plan.zh-CN.md) | 架构、风险审计、产品任务书 |
| [docs/provider-access-matrix.zh-CN.md](docs/provider-access-matrix.zh-CN.md) | Provider 接入矩阵 |
| [docs/github-release-v0.1.1.md](docs/github-release-v0.1.1.md) | v0.1.1 GitHub Release 文案 |
| [docs/green-book-integration.zh-CN.md](docs/green-book-integration.zh-CN.md) | 与 Claude Science 绿皮书联动说明 |
| [docs/troubleshooting.md](docs/troubleshooting.md) | 常见问题与排错 |

## 开发与验证

面向开发者的常用检查：

```powershell
.\scripts\self-test.ps1
Push-Location launcher
pnpm tauri build --debug --no-bundle
Pop-Location
.\scripts\package-launcher-portable.ps1 -Profile debug -SkipBuild
```

发布前至少确认：

- 前端构建通过。
- Rust 测试通过。
- Bridge self-test 通过。
- 便携包验收预览通过。
- 仓库和发布包中没有 `sk-...` 形式的真实 API Key。

## 许可

本仓库按 [LICENSE](LICENSE) 发布。若未来复制或改写第三方项目代码，需要保留对应项目的许可证和著作权声明。
