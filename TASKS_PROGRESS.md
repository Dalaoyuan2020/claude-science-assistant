# CSA T7 Merge Progress

- [x] T7-1 合并切换可靠性修复与 API Key 预选确认
- [x] T7-2 给方案一/方案二增加预选确认
- [ ] T7-3 构建、切换运行实例并验收

## T7-1

做了什么：已确认工作副本为 `codex/csa-v0.1.5-model-roles`；合并前未提交的聚合功能改动已保存到可恢复 stash `codex-pre-t7-model-roles-working-tree`。

证据在哪：`git stash list` 与当前分支状态。

下一步：cherry-pick `e871469`、`99b2219`，再恢复聚合改动并按 T0-T6 新实现优先解决冲突。

## T7-1 完成

做了什么：cherry-pick 两个修复提交；冲突处保留聚合数据结构和界面，同时采用 T0-T6 的 Bridge 启动、revision/source 校验、上游预检、日志与回滚实现；新增 API Key 继续只保存不激活。

证据在哪：合并提交 `4ab4460`、`c540b26`；`restart_bridge_after_config` 只有函数末尾的成功返回，不再有静默早退。

下一步：把同一套预选确认状态机扩展到方案一/方案二。

## T7-2 完成

做了什么：方案按钮只加载并高亮草案；新增取消与确认切换；只有确认函数调用 `activate_aggregate_scheme`；API/聚合页签只切换视图；聚合激活前逐路预检三条上游。

证据在哪：`npm run build` 通过；Rust 测试 `aggregate_scheme_ui_requires_pending_confirmation`、`api_key_ui_requires_pending_confirmation` 和切换可靠性测试通过；完整 Rust 结果为 52+9 passed、0 failed、3 ignored。

下一步：运行代理回归、构建最终 debug exe、校验时间戳和 Bridge 来源，然后关闭旧实例并启动新版。

## T7-3 进行中

做了什么：前端、Rust、代理测试全部通过；debug exe 已生成，构建时间 `2026-08-09 21:47:18`，晚于最新源码 `2026-08-09 21:46:45`。

证据在哪：`npm run build`；Rust 52+9 passed；53 translation tests passed；proxy verification passed；`docs/reports/CSA_T7_merge_status_20260809.md`。

当前阻塞：旧启动器 PID `190640` 以更高权限运行，普通关闭只进入托盘，`Stop-Process` 与 `taskkill` 两次均被 Windows 拒绝。按纪律停止强杀，没有启动第二份。

下一步：人工从旧启动器托盘“退出”或管理员任务管理器结束 PID `190640`，随后启动新版并确认 Bridge source 与合并界面。
