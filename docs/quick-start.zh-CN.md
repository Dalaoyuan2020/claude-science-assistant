# Claude Science 助手：新手接入教程

本教程面向第一次使用的 Windows 用户。目标是通过“电脑体检 Skill + Claude Science 助手”完成环境检查、模型接入和日常启动，而不是手工修改系统网络。

## 你需要准备

- Windows 10 22H2 或 Windows 11 的 64 位电脑。
- 已开启的 CPU 虚拟化能力。
- 至少 8 GB 可用系统盘空间，推荐 15 GB 以上。
- 一个你自行选择的 Provider 账号或 API Key。

Claude Science 助手不会要求修改 Clash、VPN、DNS、hosts、根证书、系统代理或 443 端口。

## 第一步：把 CSA 文件夹交给 Codex

推荐做法不是让用户自己手动执行一堆命令，而是把解压后的 CSA 文件夹作为 Codex/Codex IDE 的工作区打开，再把下面这段 Prompt 发给 Codex。

```text
请你帮我在这台 Windows 电脑上安装并启动 CSA（Claude Science Assistant）。

请先阅读当前文件夹里的 README.md、docs/quick-start.zh-CN.md、docs/v0.1-clean-pc-acceptance.zh-CN.md，以及 skills/bootstrap-claude-science-wsl/SKILL.md。

目标：
1. 帮我完成电脑体检。
2. 如果环境允许，安装/修复 CSA 在 WSL 内的运行时。
3. 启动 Claude Science 和本地 Bridge。
4. 指导我添加供应商、测试 API Key 连通性、自动映射模型。

安全要求：
1. 先只读检查和预览，不要一上来安装、删除、重启或修改系统。
2. 不要修改 Clash、VPN、DNS、hosts、系统代理、根证书或 443 端口。
3. 不要输出、保存、截图或提交我的 API Key、OAuth token、Cookie 或控制 token。
4. 任何需要管理员权限、启用 Windows 功能、安装 WSL/Ubuntu 或重启的步骤，都必须先停下来向我说明并等我确认。

执行顺序：
1. 先运行 1-run-acceptance-preview.bat，或使用 bootstrap-claude-science-wsl Skill 做只读体检。
2. 告诉我这台电脑是否已有可用 WSL/Ubuntu。
3. 如果已有 WSL/Ubuntu，请说明将要安装/修复的 CSA WSL 运行时；我确认后再运行 4-install-runtime-after-preview.bat。
4. 如果没有 WSL/Ubuntu，请不要静默安装；先说明需要管理员权限、推荐 Ubuntu 版本、可能启用的 Windows 功能和是否可能重启。只有我明确同意后，才可以使用 -InstallWslIfMissing。
5. 安装/修复后运行自测，确认没有重复 Bridge。
6. 打开 claude-science-assistant.exe，引导我添加供应商、测试 API Key、自动映射模型并启动 Claude Science。

如果失败：
1. 不要反复重装。
2. 先生成脱敏诊断摘要。
3. 告诉我失败在哪一步、可能原因、下一步需要我确认什么。
```

如果你不是用 Codex，而是自己双击脚本，也应遵守同样的顺序：先预览，再确认，再安装/修复。

## 第二步：在 Codex 中体检电脑

安装项目内的 `bootstrap-claude-science-wsl` Skill 后，对 Codex 说：

```text
请使用 $bootstrap-claude-science-wsl 检查这台电脑是否能运行 Claude Science 助手。
先只读检查，不要安装、重启或修改系统。
```

体检会检查 Windows、虚拟化、磁盘、WSL2、Ubuntu、端口、Bridge、Claude Science 和重复进程。报告只显示秘密是否存在，不会输出秘密内容。

## 第三步：确认安装计划

如果电脑还没有 WSL2，Codex 会先列出：

- 需要开启的 Windows 功能。
- 要安装的 Ubuntu 版本。
- 是否需要管理员权限和重启。
- 将创建的服务与文件路径。
- 失败时如何回滚。

只有在你明确同意后才能开始安装。Windows 要求重启时，先重启，再让 Codex重新体检并继续。

建议先让 Skill 用 `repair-approved.ps1 -PlanOnly` 预览将要执行的步骤；确认后再执行 `repair-approved.ps1 -ApproveInstall`。如果需要安装 Ubuntu，必须额外确认 `-InstallWslIfMissing`，并准备好管理员权限和重启。

如果你使用的是便携包，不需要自己运行 npm/pnpm，也不需要自己准备 Claude Science Linux 二进制。包里已内置锁定版 Linux 二进制；先双击 `1-run-acceptance-preview.bat` 预览。

如果电脑已有可用 WSL/Ubuntu，确认后再双击 `4-install-runtime-after-preview.bat` 安装/修复 CSA 在 WSL 内的运行时、启动服务并自测。

如果电脑还没有 WSL/Ubuntu，`4-install-runtime-after-preview.bat` 不会默认静默安装系统级 WSL。此时需要额外确认 `-InstallWslIfMissing`，并准备管理员权限和可能的重启；新手建议让 Codex 使用包内 Skill 引导完成。

## 第四步：添加或更换供应商

Claude Science 助手首页按添加顺序显示已保存的 API Key，并突出当前正在使用的一条。需要新增时，点击“添加供应商”，服务商模板按以下顺序排列：

1. 官方直连：GLM-5.2、LongCat、DeepSeek、MiniMax、Claude、OpenAI/GPT。
2. 聚合平台：OpenCode Go、OpenRouter。
3. 中转服务：项目方自建中转 `https://10521052.xyz/v1`，以及用户自行填写的自定义中转。

项目方自建中转由 CSA 项目方维护，但不是模型厂商官方 API，也不会自动启用。自定义中转默认留空；两者都应在确认屏幕显示的域名后再保存 API Key。

Claude 与 OpenAI/GPT 的“官方订阅登录”和“API Key”是不同入口。订阅权益不能默认当成 API 余额使用。
非 Claude 官方登录项添加时必须输入 API Key。启动器使用 Windows 当前用户 DPAPI 加密保存密钥；点击列表中的“使用”即可切换，不需要重新输入。界面只显示“已加密保存”，不会把已保存的 Key 或密文读回显示。当前正在使用的 Key 必须先切换到另一条后才能删除。

当前激活的 Key 会被应用到 WSL Bridge 的运行时配置 `~/.claude-science/proxy/config.json`，该文件应保持 `0600` 权限。这是 Bridge 调用上游模型所必需的；便携包、前端状态、日志和验收输出不会携带明文 Key。

DPAPI 密钥只属于当前 Windows 用户和当前电脑。把便携包复制到另一台电脑时，不会带走这些 API Key；需要在新电脑上重新添加。

如果你从早期测试版升级，首页 API Key 列表可能为空。这是正常的：早期版本只保存当前服务商选择，没有保存可迁移的加密 Key 列表；请重新点击“添加供应商”。

需要高级路由、模型别名或连接测试时，先启动 Bridge，再点击启动器底部的“配置面板”。如果 Bridge 启用了 required path secret，启动器会打开带本地 path secret 的面板地址，但不会把 token 显示在界面上。

## 第五步：启动

环境就绪后，首页应显示：

- WSL2 正常。
- 运行时已准备。
- Bridge 健康。
- Claude Science 正在运行。
- WSL 存储显示实际 VHDX 宿主位置和剩余空间。
- 当前 API Key 对应的服务商名称。

点击“打开 Claude Science”。日常使用不需要重新安装，也不需要打开终端。

如果“WSL 存储”显示在 C 盘或宿主盘空间不足，v0.1.3 r2 会显示“推荐迁移 / 辅助迁移”。点击后复制本机化 Prompt 给 Codex；Codex 会先只读检查 WSL 版本、发行版 GUID、默认用户、systemd、目标盘和备份空间，再输出计划并停下。这个按钮不会自动搬迁发行版，也不会执行 `unregister`；不要手工移动 `ext4.vhdx`。

从旧版升级时也不需要重装环境：下载完整新 ZIP 到新目录，让新版接管旧 Bridge，确认六项状态和真实对话正常后再删除旧目录。这是并排增量升级，不是只替换一个 EXE。

如果启动器提示“环境尚未就绪”，不要反复双击启动器；先回到解压目录运行 `1-run-acceptance-preview.bat`，确认计划后再运行 `4-install-runtime-after-preview.bat`。这样可以避免因为缺运行时或版本不兼容造成看起来像“闪退/循环弹窗”的体验。

## 第六步：连接飞书或 Telegram

在启动器下方切换到 `Connect`：

1. 飞书推荐点击“扫码创建飞书机器人”，通过飞书官方一次性页面自动创建应用、权限和长连接；已有企业自建应用也可手动填写 App ID/App Secret。旧 Incoming Webhook 仍可发通知，但不能用于双向对话。
2. Telegram 使用 BotFather 提供的 Bot Token，不再手填 Chat ID。
3. 保存后点击“配对”。Telegram 会打开带 `/start` 参数的机器人深链，也可以扫描同一页面的二维码；手工 `/pair <code>` 只作为降级方式。
4. 配对成功后先发一条普通消息，再回到面板“项目路由”绑定 Claude Science 工作区。
5. 保持 Claude Science 页面打开并显示“页面就绪”。普通消息会自动注入当前真实会话，回复经 Bridge 自动回到原聊天线程。
6. `安装 Skill` 与 MCP 连接信息用于手动领取或浏览器不可用时的降级，不是正常自动闭环的必经步骤。

### 可选：安装浏览器插件增强投递

如果 Claude Science 正在 Chrome 中打开，可以在 `Connect` 页面安装 `CSA Claude Science Page Connector` 插件。它只负责页面内定位 Notebook 输入框、投递消息和回传页面状态，不提供聊天窗，也不会执行本地命令。

1. 点击“浏览器插件增强”里的“安装插件”。
2. Chrome 打开 `chrome://extensions/` 后，开启“开发者模式”。
3. 点击“加载已解压的扩展程序”，选择面板复制的 `extensions/csa-claude-science-connector` 目录。
4. 回到 CSA 点击“连接插件”；插件心跳会在十分钟窗口内自动完成配对，手工输入配对码只作为降级方式。
5. 打开本机 Claude Science 页面后，状态显示“页面就绪”，Connect 本地输入会优先通过插件投递；插件不可用时仍会回落到 UIAutomation 或队列。

关闭主窗口后 CSA 会留在系统托盘继续接收消息；从托盘选择“退出”才会停止 Connect Gateway。首版不会自动唤醒空闲的 Claude Science 会话，也不允许聊天端直接执行系统命令。完整技术说明和测试案例见 [connect-gateway-implementation.zh-CN.md](connect-gateway-implementation.zh-CN.md)。

飞书与 Telegram 的实机步骤、半流式验收和故障定位见 [connect-live-test-guide.zh-CN.md](connect-live-test-guide.zh-CN.md)。

## 出现问题时

先点击“刷新状态”，再让 Codex 使用体检 Skill 生成脱敏报告。使用便携包测试时，可以在解压目录直接双击 `2-collect-acceptance-evidence.bat` 生成脱敏证据包；分享前仍建议手工打开看一眼，确认没有 API Key 或 token。

如果你更习惯命令行，也可以运行保留的 PowerShell 原版：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\collect-acceptance-evidence.ps1
```

不要通过反复安装、关闭安全软件或修改系统代理来碰运气。

若报告发现 Windows 与 WSL 各有一份 Bridge，应先迁移为 WSL 单实例；在完成迁移前，不要把两个健康页面都当成同一个服务。

需要卸载或回滚时，也先让 Skill 运行 `rollback-approved.ps1 -PlanOnly` 预览。确认后再执行 `-ApproveUninstall`；默认会保留 Provider 配置、API Key、OAuth token 和原始 Claude Science。
