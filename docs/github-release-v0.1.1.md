# CSA v0.1.1 — Claude Science Assistant

> 首个 GitHub 公开版本：面向 Windows 用户的 Claude Science 启动器、WSL Bridge 管理器与国产模型接入助手。

CSA 是 [Claude Science 绿皮书](https://github.com/Dalaoyuan2020/claude-science-green-book) 第二章“装上你的科研搭档”的配套工具。它的目标不是让读者学习系统运维，而是帮读者把 Claude Science 跑起来，并接入 GLM、LongCat、DeepSeek、MiniMax、OpenCode Go、OpenRouter 或自定义中转。

## 下载

请只从本仓库 GitHub Releases 下载：

- `claude-science-assistant-v0.1.1-release-portable.zip`
- `claude-science-assistant-v0.1.1-release-portable.zip.sha256`

发布时请同时上传同名 `.sha256` 文件，并在 GitHub Release 页面复制其中的值：

```text
SHA256: <从 claude-science-assistant-v0.1.1-release-portable.zip.sha256 复制>
```

不要从网盘、群文件或第三方镜像下载带 `claude-science-assistant.exe` 的压缩包。

## 这一版解决了什么

- 把 CSA 定位为 Windows 上的 Claude Science 启动器，而不是一堆散落脚本。
- 首页只显示当前 API Key、当前状态和启动动作；新增 Key 放到“添加 API Key”入口里选择。
- API Key 按添加顺序保存，一次只激活一条，避免所有 Provider 平铺到主面板。
- 添加“测试 API Key”和“自动映射”能力，减少跑到项目内部试错。
- 修正 DeepSeek 兼容：
  - 默认 `deepseek-v4-pro`
  - 快速/Haiku 映射 `deepseek-v4-flash`
  - 兼容旧名或误填：`Deep-chat`、`deep-chat`、`deepseek-chat`、`deepseek-reasoner`
- 新增或修正 Provider 模板：
  - GLM-5.2
  - LongCat：`LongCat-2.0`
  - MiniMax：`MiniMax-M3` / `MiniMax-M2.7-highspeed`
  - OpenCode Go：默认 `glm-5.2`，主力优先 `glm-5.2` → `qwen3.7-max` → `deepseek-v4-pro`，快速 `deepseek-v4-flash`
  - OpenRouter
  - 内置第三方中转 `https://10521052.xyz/v1`
  - 自定义 OpenAI-compatible 中转
- WSL 策略从“硬锁 Ubuntu-24.04”改为“推荐 Ubuntu-24.04，兼容已安装 Ubuntu/WSL 发行版”。
- 保留 PowerShell 脚本，同时提供可双击的 BAT 入口，降低新手门槛。

## 推荐使用流程

1. 下载 zip，并完整解压。
2. 推荐把解压后的 CSA 文件夹作为 Codex/Codex IDE 工作区打开，把 README 或 `docs/quick-start.zh-CN.md` 里的安装 Prompt 发给 Codex Agent。
3. 先双击 `1-run-acceptance-preview.bat`，或让 Codex 使用包内 Skill 做只读预览。
4. 如果电脑已有可用 WSL/Ubuntu，确认后双击 `4-install-runtime-after-preview.bat`。它会安装/修复 CSA 的 WSL 运行时、启动服务并自测。
5. 如果电脑没有 WSL/Ubuntu，不要把第 4 步当成静默系统安装器。需要额外确认 `-InstallWslIfMissing`，并准备管理员权限和可能的重启；新手建议让 Codex 使用包内 Skill 引导完成。
6. 双击 `claude-science-assistant.exe`。
7. 点击“添加 API Key”，选择服务商模板。
8. 输入 Key，测试连通；如果有多个模型，点击“自动映射”。
9. 保存并启动 Claude Science。

简化理解：CSA 是便携启动器；`4-install-runtime-after-preview.bat` 能安装/修复 CSA 在 WSL 内的运行时，但首次安装 WSL/Ubuntu 属于系统级操作，需要额外确认。不要反复双击 exe 碰运气。

## Provider 默认顺序

1. 官方直连：GLM-5.2、LongCat、DeepSeek、MiniMax、Claude、OpenAI / GPT
2. 聚合平台：OpenCode Go、OpenRouter
3. 第三方中转：内置 `https://10521052.xyz/v1`、自定义 Base URL

第三方中转不会默认信任。CSA 会要求确认域名后，才会把 API Key 发往该地址。

## 自动映射逻辑

CSA 会把 Claude 侧模型分成主力和快速两类：

| Claude 侧角色 | 映射意图 |
| --- | --- |
| `byok-model-0001` | 主力模型 |
| `claude-sonnet-*` | 主力模型 |
| `claude-opus-*` | 主力模型 |
| `claude-haiku-*` | 快速/低延迟模型 |

如果 API Key 的 `/models` 只能读取到一个聊天模型，CSA 会把所有 Claude 角色统一映射到这个模型；如果有多个模型，则优先选择 Pro/Max/大型模型作为主力，Fast/Flash/Highspeed/Mini/Lite/Air 作为快速模型。

## 安全说明

- CSA 不会把明文 API Key 写入发布包、README、Release 说明或日志。
- API Key 使用 Windows 当前用户 DPAPI 加密保存。
- DPAPI 绑定当前 Windows 用户和当前电脑；复制便携包到另一台电脑不会带走 API Key。
- 当前激活 Key 会写入 WSL Bridge 运行时配置，供本地 Bridge 调用上游模型；该配置应保持 `0600` 权限。
- CSA 默认不修改 Clash、VPN、DNS、hosts、系统代理、根证书或 443 端口。

## 已知限制

- v0.1.1 是首个公开版本，建议先在测试电脑或可回滚环境中验证。
- 第三方中转的稳定性、模型列表和响应格式取决于上游平台。
- OpenRouter 等动态模型平台建议先测试连通，再保存映射。
- 如果上游返回 HTTP 200 但内容为空，通常是模型不可用、限流、余额或中转兼容问题，不一定是 CSA 本地映射失败。

## 文档

- 新手教程：[docs/quick-start.zh-CN.md](quick-start.zh-CN.md)
- 架构与产品任务书：[docs/architecture-and-product-plan.zh-CN.md](architecture-and-product-plan.zh-CN.md)
- Provider 矩阵：[docs/provider-access-matrix.zh-CN.md](provider-access-matrix.zh-CN.md)
- 绿皮书联动：[docs/green-book-integration.zh-CN.md](green-book-integration.zh-CN.md)
