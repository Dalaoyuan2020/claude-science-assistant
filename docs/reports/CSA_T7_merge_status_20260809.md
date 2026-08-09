# CSA T7 合并状态报告

日期：2026-08-09

分支：`codex/csa-v0.1.5-model-roles`

## 当前结论

T7-1 与 T7-2 已完成。T7-3 的 Bridge 来源和 API Key 真实切换已通过，但聚合方案验收发现状态污染，按任务纪律停止，T7-3 不通过且未勾选。

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
- 新 debug 启动器 PID `190080` 正常运行。
- Bridge `source_path` 已指向 `/mnt/c/Users/Admin/Documents/New project 5/csa-v0.1.5-model-roles/proxy.py`，不再指向 v0.1.4。
- API Key 连续预选三项时，Bridge PID `4045196` 和 revision `190640-1786281097675014600` 均保持不变。
- 只确认一次后，Bridge 仅重启一次到 PID `4046269`，revision 仅更新一次为 `190080-1786288134669273000`。
- 切换后的真实请求成功，Bridge 最近后端记录为 `custom / LongCat-2.0 / success`。

## 当前阻塞

API Key 确认切换到 `LongCat` 后，聚合页的方案一草稿被自动改成三个 `LongCat`，界面显示“方案一有未应用修改”。此时方案二无法被预选，“确认切换”按钮不可用。

这说明 API 激活与聚合方案草稿之间仍有非预期状态耦合。由于方案一/方案二无法完成“连续预选零重启、确认仅重启一次”的验收，T7-3 必须保持未完成。现场没有继续改代码或强行保存方案。

## 待执行

1. 定位 API 激活后聚合方案草稿被当前 API 覆盖的触发路径。
2. 修复时确保 API Key 激活只更新当前 API 接入，不写入任何聚合方案 routes 或草稿。
3. 从 API 切换完成后的现场重测方案二、方案一、方案二预选，确认 PID/revision 不变。
4. 只确认一次目标方案，确认 Bridge PID/revision 各变化一次，并验证三条模型路由。
