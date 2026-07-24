# CSA v0.2.0

CSA v0.2.0 把启动器从单纯的本机运行环境管理器扩展为轻量科研连接与任务中枢。本版本新增 Connect 双向消息链路和 Subagent 沙盒外 Agent 请求闭环，同时保留人工审批与本地优先边界。

## 主要更新

### Connect

- Telegram Bot API 长轮询与飞书企业应用长连接。
- 本地 Go Gateway、SQLite 持久队列、配对、项目绑定和只读审计历史。
- Claude Science MCP Connector 与 `csa-connect` Skill。
- Chrome Manifest V3 页面连接器，优先把消息投递到当前 Claude Science composer。
- UIAutomation 和工作区队列回退。
- 分阶段进度回复、消息去重和 `delivery_unknown` 防重复策略。
- Claude Science artifact 图片校验后发送到 Telegram。

### Subagent

- Claude Science 通过 `csa-external-agent` Skill 提交脱敏请求。
- 启动器面板人工批准后打开本机 Claude Code plan 会话。
- 支持 session 恢复、只读会话历史和继续追问。
- 稳定的 inbox、runs、outbox 文件接口。
- 请求和结果路径校验、凭据样式拦截与摘要脱敏。

### 发布与升级

- 新功能统一验证器 `scripts/verify-new-features.ps1`。
- 完整便携包包含 Bridge、锁定版 Claude Science、Connect Gateway、两个 Skill、浏览器插件、测试与文档。
- 支持 v0.1.x 并排升级和旧目录回退，不覆盖旧安装。

## 安装

1. 下载完整 ZIP 和 `.sha256`，校验后解压到新目录。
2. 先运行 `1-run-acceptance-preview.bat`。
3. 再运行 `4-install-runtime-after-preview.bat` 安装或修复 WSL 运行时。
4. 使用 `3-open-claude-science-assistant.bat` 打开启动器。

不要只下载或替换 EXE。API Key 和平台凭据不包含在发布包中。

详细步骤见 `docs/v0.2-install-upgrade-release-guide.zh-CN.md`。

## RC 验收说明

`v0.2.0-rc.1` 已通过自动测试、真实 Claude Code 两轮续聊、Gateway/MCP 与 Bridge 本机链路测试。正式稳定版发布前仍需完成真人 Telegram 文本和图片闭环复验。飞书需要在实际企业租户内单独验收。

## 安全边界

- 外部聊天文字永不作为 shell 命令执行。
- Subagent 默认且强制经过本地人工批准。
- 不修改 VPN、系统代理、DNS、hosts、证书或 443 端口。
- 不自动迁移、注销或重建 WSL 发行版。
- 凭据使用 Windows 当前用户 DPAPI 和 WSL `0600` 配置保存，不进入日志或发布包。
