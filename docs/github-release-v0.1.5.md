# CSA v0.1.5

CSA v0.1.5 基于 v0.1.4 稳定核心，继续锁定 Claude Science 0.1.25，并完成多订阅三模型聚合与切换可靠性加固。

## 下载

- `claude-science-assistant-v0.1.5-publish-20260810-release-portable.zip`
- `claude-science-assistant-v0.1.5-publish-20260810-release-portable.zip.sha256`

请只从本仓库 GitHub Release 下载完整 ZIP，并用同页 `.sha256` 文件核验。不要只复制 EXE。

## 新增

- 多订阅角色映射：把决策（default / Opus）、视觉（vision / Sonnet）和快速（fast / Haiku）分别绑定到订阅与模型。
- 方案一与方案二：每套方案保存完整的三路映射，一次切换整套配置。
- 预选 + 确认式切换：点击 API Key 或方案只会高亮待生效项，点击“确认切换”后才执行一次事务化切换。
- 添加供应商时可读取模型列表，并在自动建议后手动修改三层模型。
- API Key 列表最多显示五条，更多订阅在列表内部滚动；API 接入区可折叠。

## 修复

- 消除 Bridge 未运行时直接返回成功的静默路径。
- 切换后校验 `source_path`、配置 revision 与真实上游请求；任一步失败都会显示错误并回滚。
- API Key 预检进程增加 20 秒外层硬超时，避免界面长期停在“正在处理”。
- 发布包保留可审计的前端产物和 self-test 所需安全检查材料。

## 已知限制

- 裸切 Provider 时，如果模型别名集合发生变化，Claude Science 本体可能需要重启后才会刷新模型列表；使用角色映射可避免这一问题。
- 当前 `verify-proxy.ps1` 只识别单后端配置，不能正确判断合法的 aggregate-only 三路配置；源码测试、真实方案切换和包内 self-test 已通过，此脚本兼容项保留待修。
- 在很深的 Windows 解压路径运行 self-test 可能触发传统路径长度限制；建议解压到 `D:\CSA` 等短路径。启动器本身不依赖该测试虚拟环境。

## 首次安装

1. 下载完整 ZIP 和 `.sha256`，核验后解压到新目录。
2. 推荐解压到短路径，例如 `D:\CSA-v0.1.5`。
3. 阅读 `docs/quick-start.zh-CN.md`，或把 `docs/prompts/csa-install-or-upgrade-agent-prompt.zh-CN.md` 交给本地 Codex 先做只读体检。
4. 双击 `claude-science-assistant.exe`，按界面添加供应商并测试连接。

## 从旧版升级

1. 不卸载 WSL、Ubuntu 或 Claude Science 数据。
2. 把 v0.1.5 完整 ZIP 解压到新目录，不覆盖旧目录。
3. 关闭旧启动器后运行新目录的 EXE；新版会读取同一 Windows 用户已有的 DPAPI 设置。
4. 让新版接管 Bridge，确认 `source_path` 指向 v0.1.5 目录，并完成一次真实模型请求。
5. 保留旧目录作为回退，验收稳定后再删除。

## 校验值

`claude-science-assistant-v0.1.5-publish-20260810-release-portable.zip`

```text
SHA256 82D8AA53BEAEA4D09E1E3AA41B318AD86675F3976EFA6688CD87CF8CFDF9814A
```
