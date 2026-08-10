# CSA v0.1.5 发布报告

人工只需做什么：下载 GitHub Release 的 ZIP 与 `.sha256`，解压到短的新目录，在一台干净或旧版升级电脑上打开启动器；确认版本号为 V0.1.5，分别完成一次视觉订阅图片请求和一次旧版并排升级。其余源码、真实方案切换、包内自检、凭据扫描与 Release 发布均已自动完成或留下明确证据。

## R0-R1：发布前复盘

- 工作副本：`csa-v0.1.5-model-roles`
- 分支：`codex/csa-v0.1.5-model-roles`
- 前端：34 modules，构建通过。
- Rust：52 个库测试 + 9 个集成测试通过，0 failed，3 ignored。
- Bridge self-test：53 translation tests passed。
- 真实切换 scheme-2 -> scheme-1：54.8448143 秒，三路聚合、revision 和真实响应均成功。
- 真实切换 scheme-1 -> scheme-2：48.9163934 秒，三路聚合、revision 和真实响应均成功。
- 待人工：`verify-proxy.ps1` 只识别单后端 `custom/deepseek/openai_configured`，不识别合法的 `aggregate_upstreams=3` 配置。

## R2-R3：最小加固与回归

- 在 `launcher/src-tauri/src/lib.rs` 新增可通过 stdin 传递敏感输入的 PowerShell 进程超时封装。
- 只给 `test_api_key_impl` 增加 20 秒外层硬超时；内部 HTTP 45 秒超时、`CSA_BRIDGE_ONLY=1` 和切换预检保持不变。
- 加固后前端仍为 34 modules；Rust 52 + 9 passed、0 failed、3 ignored；self-test 仍为 53 passed。
- 未发生回退。

## R4：提交与待人工项

- `037968d feat: add aggregate subscription schemes`
- `8a4067b fix: bound API key preflight execution`
- `f5edf38 feat: display the v0.1.5 launcher version`
- `fcb59f2 docs: record the v0.1.5 release workflow`
- 待人工：六份互相冲突的 `CSA_T8*20260809.md` 草案保留为本地未跟踪文件，未进入发布提交。

## R5：发布包

- 路径：`dist/release-v0.1.5-publish-20260810/claude-science-assistant-v0.1.5-publish-20260810-release-portable.zip`
- 大小：90,677,168 字节。
- SHA-256：`82D8AA53BEAEA4D09E1E3AA41B318AD86675F3976EFA6688CD87CF8CFDF9814A`
- 内置运行时：Claude Science 0.1.25，运行时 SHA-256 与仓库 manifest 一致。

## R6：包上检测

1. 时间戳：通过。最新打包源码为 `2026-08-10T01:28:22+08:00`，包内 EXE 为 `2026-08-10T01:30:24+08:00`。
2. 前端 JS：通过。包内 `index-Bakxun1r.js` 对“确认切换”“方案一”“方案二”各命中 1 次。
3. 包内 self-test：通过。最终 ZIP 解压到短路径后得到 `53 translation tests passed` 和 `self-test passed`。
4. SHA-256：通过。`.sha256` 声明与独立复算均为 `82D8AA53BEAEA4D09E1E3AA41B318AD86675F3976EFA6688CD87CF8CFDF9814A`。

直接在超长开发路径运行 self-test 会触发 Windows 传统路径长度限制；短路径复测完整通过。

## R7：安全与发布

- 凭据扫描：仓库 120 个文本文件、包内 64 个文本文件；Telegram、OpenAI、Anthropic、GitHub 与 Bearer 凭据模式均为 0 命中。
- 包内未包含 `.env`、真实 `config.json`、数据库或日志；`setup-token.py` 是公开工具源码，不含凭据。
- GitHub tag：`v0.1.5`。
- Release 链接：<https://github.com/Dalaoyuan2020/claude-science-assistant/releases/tag/v0.1.5>
- 页面更新：README 增加 v0.1.5 下载入口、订阅列表表格、角色/方案绑定表格、安装与升级入口。
