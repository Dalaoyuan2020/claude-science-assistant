> ⚠️ 已被 CSA_v015_release_tasks_R0R7_20260810.md 取代。
# 触发指令 · v0.1.5 发布流程 (2026-08-10)

读 `docs/plans/CSA_v015_release_flow_20260810.md` 并**一路执行到底，中途不要停下来等人确认**。吕博士稍后直接验收。

功能已经可用（实测：聚合 3 条上游、scheme-2 生效、切换由启动器写入）。本次是**收尾发布**，不是继续修 bug。

## 六步
1. **P0 复盘检测**：npm build / cargo test / self-test / verify-proxy 全跑，加两次方案切换实测，记实际秒数
2. **P1 最小加固**：只给预检加 20 秒进程超时（`test_api_key_impl` 里 `run_powershell_with_stdin` → `command_output_with_timeout`）。**其余一律不动**——不要碰 `CSA_BRIDGE_ONLY`、不要改 45 秒超时、不要删预检调用
3. **P2 提交**：41 项未提交改动分类，环境/构建产物不提交，判断不了的标「待人工」
4. **P3 打包**：用 `scripts\package-launcher-portable.ps1`，出 release 版，命名 `release-v0.1.5-publish-20260810`
5. **P4 包上检测**：必须在**包**上验，不是源码上。查 exe 时间戳、包内前端是否含「确认切换/方案一/方案二」、出 sha256
6. **P5/P6 推 GitHub**：推分支、打 tag `v0.1.5`、建 Release 传包、更新 README 和页面

## 三条硬约束
- **推送前必须自查没有任何 Key/token/密码进仓库或包**。这条不过就停下标「待人工」，不要硬推。
- 不要 kill 正在运行的启动器 GUI（权限不够）。
- 遇卡点记台账标「待人工」后**继续做剩下的**，不要停在那里干等。

工作副本 `csa-v0.1.5-model-roles`，台账 `TASKS_PROGRESS.md` 追加发布流程。
