# CSA 首次安装 / 升级通用 AI Prompt

把 CSA 完整 ZIP 解压后，将解压目录作为 Codex、Claude Code 或其他具备本机命令能力的 AI 助手工作区，然后发送以下 Prompt。

```text
请协助我在当前 Windows 电脑上安装或升级 CSA（Claude Science Assistant）。当前文件夹是我刚解压的新版 CSA 完整便携包。

先阅读：
- README.md
- docs/quick-start.zh-CN.md
- docs/v0.1-clean-pc-acceptance.zh-CN.md
- skills/bootstrap-claude-science-wsl/SKILL.md
- manifest.json

必须遵守：
1. 第一阶段只做只读体检，不安装、不删除、不停止服务、不重启、不修改系统。
2. 不修改 Clash、VPN、DNS、hosts、Windows 系统代理、证书或 443 端口。
3. 不输出、记录、截图、上传或提交 API Key、OAuth token、Cookie、DPAPI 密文或 Bridge 控制 token。
4. 不注销、导出、迁移、压缩、删除或重建任何 WSL 发行版。
5. 不清空 `%APPDATA%\ClaudeScienceAssistant`、`~/.claude-science` 或 `~/.local/share/claude-science-api-bridge`。
6. 不要只替换 EXE，也不要把新版文件零散覆盖到正在使用的旧目录。

第一阶段：只读识别场景
请检查并报告：
- Windows 版本、架构、可用磁盘空间；
- WSL 是否安装、发行版名称、WSL1/WSL2、默认用户、systemd；
- 当前文件夹的 manifest 版本、SHA256 文件和关键目录是否完整；
- 是否存在旧版 CSA 解压目录或正在运行的 CSA 启动器；
- 9876、8765、8766 端口状态；
- Bridge `/health`、`source_path` 和 Claude Science 状态；
- `%APPDATA%\ClaudeScienceAssistant\settings.json` 是否存在，只报告存在与否，不读取或输出 Key/密文；
- WSL 中 `~/.claude-science/proxy/config.json` 和 CSA 管理运行时是否存在，只报告存在与权限；
- WSL 虚拟磁盘所在宿主盘及剩余空间。

然后只允许得出以下一种场景：
A. FIRST_INSTALL_NO_WSL：没有可用 WSL/Ubuntu；
B. FIRST_INSTALL_EXISTING_WSL：有可用 WSL，但没有 CSA 持久设置和运行时；
C. UPGRADE_SAME_USER：同一台电脑、同一 Windows 用户已有 CSA 设置或旧运行时；
D. AMBIGUOUS_OR_BROKEN：证据矛盾、文件系统只读、端口归属不明或无法安全判断。

先向我输出：场景、证据、计划、会修改的路径、是否需要管理员权限、是否可能重启、回滚办法。没有得到我的明确确认，不得进入第二阶段。

第二阶段规则：

如果是 A：
- 先说明安装 WSL/Ubuntu 会启用哪些 Windows 功能、是否需要管理员权限和重启；
- 推荐 Ubuntu-24.04，但不要因为用户已有兼容 Ubuntu 就强制重装；
- 只有我明确同意后才安装 WSL；重启后重新从第一阶段体检。

如果是 B：
- 不重装现有 WSL/Ubuntu；
- 使用包内 Skill 或经过预览的安装脚本安装 CSA WSL 运行时；
- 启动后检查 Bridge、Claude Science、存储和端口，不自动填入任何模型或 Key。

如果是 C：
- 执行并排升级，不卸载 WSL/Ubuntu，不删除旧目录，不清空设置；
- 确认当前文件夹是完整 ZIP 解压目录，不是单独 EXE；
- 关闭旧版 Windows 启动器窗口；旧 Bridge 可以保留到接管动作开始；
- 从新版目录启动并接管 9876 端口上的旧 CSA Bridge；只允许停止能够证明是 CSA `proxy.py` 的旧监听进程；
- 验证新 Bridge 的 `source_path` 指向当前新版目录，并验证配置版本和健康状态；
- 确认启动器仍能看到原有 Key 条目名称，但不得输出 Key 内容；
- 完成一次 API 连通测试和一次 Claude Science 对话；
- 验收成功前保留旧目录。失败时停止新版并恢复旧版，不做 WSL 迁移或重装。

如果是 D：
- 停止修改；
- 生成脱敏诊断摘要，明确阻断点和需要我确认的信息；
- 不通过反复重启、杀未知进程或重装环境来碰运气。

最终验收必须包含：
- 当前包版本；
- Bridge 与 Claude Science 是否运行；
- Bridge `source_path` 是否属于当前包；
- 六项状态是否正常；
- 当前 Provider 名称是否保留；
- API 连通测试与真实对话是否成功；
- 是否保留了可回退的旧目录；
- 所有操作中是否没有输出或复制秘密。

请先执行第一阶段，只给我只读体检结果和待确认计划。
```

## 使用说明

- 新电脑和升级电脑使用同一条 Prompt，由 AI 根据证据分流。
- 用户仍需要下载完整 ZIP；只下载 EXE 无法更新 Bridge、脚本、Skill 或内置运行时。
- 同电脑、同 Windows 用户升级通常不需要重填 Key，因为启动器设置位于 APPDATA，且由 DPAPI 绑定当前用户和电脑。
- 换电脑或换 Windows 用户时，按首次安装处理并重新添加 Key。
