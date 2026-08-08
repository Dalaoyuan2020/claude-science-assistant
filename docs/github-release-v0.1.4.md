# CSA v0.1.4

CSA v0.1.4 回到 `v0.1.3-r2` 的稳定核心，并将受管 Claude Science runtime 更新到官方 stable `0.1.25`。

## 主要变化

- 锁定并校验 Claude Science 0.1.25 Linux x64 runtime。
- 保留 CSA Bridge 的 BYOK/API Key 能力。
- 修复 API Key 切换偶发失败：切换只事务化重启 Bridge，不再打断 Claude Science daemon 或当前会话。
- 默认关闭上游后台自动更新，避免未验证构建覆盖本地补丁。
- 增加 SQLite 一致性备份和只读数据库完整性验证工具。
- 增加官方 latest/stable 检查，以及交给本地 Codex 的升级/回退 Prompt；不会静默替换运行时。
- 不启用 v0.2.0 的 Connect、Telegram、浏览器插件或 Subagent 功能。

## 版本边界

- CSA 已验证推荐版：Claude Science `0.1.25`。
- 2026-08-08 官方 latest/stable：`0.1.27`；它尚未被本包自动启用。
- `0.1.21` 只作为历史重要节点展示，未提供未经校验的一键回退。

## 验证

- Rust：41 项库测试 + 9 项 Bridge 回归测试通过；在线官方索引测试通过。
- Bridge：51 项翻译测试通过。
- 真实代理链路：health、models、messages、recent requests 通过。
- API Key 配置切换：Bridge PID 更新且 Claude Science PID 保持不变。
- 0.1.25 二进制 SHA-256 与包内 manifest 一致。

首次安装仍推荐使用完整 Release ZIP。不要把新文件覆盖到旧目录；旧版升级时保留原包作为回退点，并让启动器复用当前 WSL 数据和 Windows DPAPI 设置。
