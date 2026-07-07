# CSA v0.1.2 — Bridge detection portability fix

这是 CSA（Claude Science Assistant）的第二个公开 Release，重点修复异地电脑/不同 WSL 环境下的 Bridge 状态误判问题，并补齐发布包完整性检查。

## 下载

请只从本仓库 GitHub Releases 下载：

- `claude-science-assistant-v0.1.2-release-portable.zip`
- `claude-science-assistant-v0.1.2-release-portable.zip.sha256`

发布时请同时上传同名 `.sha256` 文件，并在 Release 页面复制其中的值：

```text
SHA256: <从 claude-science-assistant-v0.1.2-release-portable.zip.sha256 复制>
```

## 这一版修复了什么

- 修复 Bridge 健康检测误判：不再把 `/health` 成功与某个脆弱的进程命令行正则绑定。
- 适配 Ubuntu-22.04 / WSL2 / `wslrelay.exe` 等异地电脑场景：只要本地 Bridge 健康检查通过，UI 就能正确识别 Bridge 运行状态。
- 新增 `scripts/status-probe.ps1`，作为 UI 之外的“裁判线”诊断脚本，用于判断：
  - Bridge 是否健康；
  - Claude Science 是否运行；
  - systemd unit 是否指向当前解压目录；
  - 9876 / 8765 / 8766 等端口证据是否一致。
- 修复 Windows 路径转 WSL 路径时反斜杠导致的跨盘符/异地路径问题。
- 修复旧版解压目录残留时，systemd service 可能仍指向旧目录的问题；启动脚本会检查并按当前项目目录修复。
- Release ZIP 打包时强制要求包含 `vendor/claude-science/linux-x64/claude-science`，同时排除 `.rar` / `.zip` / `.7z` 临时压缩包。

## 推荐使用流程

1. 下载并完整解压 `claude-science-assistant-v0.1.2-release-portable.zip`。
2. 推荐把解压后的 CSA 文件夹作为 Codex / Claude Code 等 AI 编程助手工作区打开。
3. 让 AI 助手先读取 `README.md`、`docs/quick-start.zh-CN.md` 和 `skills/bootstrap-claude-science-wsl/SKILL.md`。
4. 先做只读体检/预览，不要直接修改系统。
5. 如果已有可用 WSL/Ubuntu，再确认执行 CSA WSL runtime 安装/修复。
6. 双击 `claude-science-assistant.exe`。
7. 点击“添加供应商”，选择服务商模板，输入 Key 后测试连通。
8. 如果上游返回多个模型，再使用“自动映射”生成模型映射草案。

## 安全说明

- Git 源码仓库不提交用户 API Key、OAuth token、Cookie 或控制 token。
- Release ZIP 包含启动器、脚本、Skill、文档、Bridge 依赖以及 Claude Science Linux 二进制。
- 用户自己的 API Key 仍由本机 DPAPI/WSL runtime 配置管理，不会随便携包迁移到其他电脑。
- 如果提交 issue 或截图，请先检查是否含有真实 API Key 或私密路径。

## 验证建议

下载 ZIP 后可用同名 `.sha256` 文件核对哈希。解压后至少确认以下文件存在：

```text
claude-science-assistant.exe
scripts/status-probe.ps1
scripts/start-claude-science-wsl.ps1
scripts/start-claude-science-wsl.sh
skills/bootstrap-claude-science-wsl/scripts/inspect-wsl.sh
vendor/claude-science/linux-x64/claude-science
vendor/claude-science/linux-x64/manifest.json
```
