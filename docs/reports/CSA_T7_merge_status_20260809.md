# CSA T7 合并状态报告

日期：2026-08-09

分支：`codex/csa-v0.1.5-model-roles`

## 当前结论

T7-1 与 T7-2 已完成。T7-3 的代码构建已完成，现场运行实例切换被 Windows 权限阻塞，尚未启动第二份程序。

## 已完成

- 合入确定性 API Key 切换修复与预选确认 UI。
- 保留三模型聚合、方案一/方案二、五条滚动列表和供应商模型选择。
- 方案切换改为“点击预选，确认后只激活一次”。
- API/聚合页签只切换视图，不再隐式重启。
- 聚合方案激活前逐路预检决策、视觉、日常三条上游。
- `npm run build` 通过。
- Rust 测试为 52 个库测试和 9 个回归测试通过，0 失败，3 个外部测试按设计忽略。
- `self-test.ps1` 为 53 translation tests passed。
- `verify-proxy.ps1` 为 proxy verification passed。
- debug exe 构建完成，时间戳晚于最新源码。

## 当前阻塞

旧启动器 PID `190640` 以更高权限运行。普通关闭请求只进入托盘；`Stop-Process` 与 `taskkill` 都返回 `Access is denied`。根据任务纪律，没有继续强杀，也没有同时启动新版，避免两个启动器争用 Bridge 和写乱配置。

旧 Bridge 当前仍指向 `csa-v0.1.4-runtime-update/proxy.py`。旧启动器退出后，需要先用 model-roles 脚本执行 Bridge-only 重启，再启动新版并确认 `/health.source_path` 指向 `csa-v0.1.5-model-roles/proxy.py`。

## 待执行

1. 从旧启动器托盘选择“退出”，或以管理员权限结束 PID `190640`。
2. 重启 model-roles Bridge。
3. 启动新 exe。
4. 验证界面同时出现方案一/方案二和“确认切换”。
