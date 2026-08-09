# CSA · 把切换修复合并到 model-roles 分支（T7）

> 2026-08-09 Napoleon 写入。**这是一次合并任务，不是重做。**
> 起因：v3 任务单把工作副本指向了 `csa-v0.1.4-runtime-update`，但吕博士在用的聚合功能（方案一/方案二）在 `csa-v0.1.5-model-roles` 分支上。结果切换修复做在了没有聚合功能的那条线上。**这是任务单的错，不是你的错，代码本身没问题。**

## 现状（已核实，不用再查）

| 分支 | 有什么 | 缺什么 |
|---|---|---|
| `codex/csa-v0.1.4-runtime-update` @ `ac21a54` | ✅ 切换可靠性修复 + 预选确认 UI | ❌ 聚合/方案一方案二 |
| `codex/csa-v0.1.5-model-roles` @ `2e57c99` | ✅ 聚合、scheme-1/scheme-2、角色绑定 | ❌ 切换可靠性修复、预选确认 UI |

要合并的两个 commit（第三个 `ac21a54` 是验收记录，可选）：
- `e871469` fix: make API key switching deterministic
- `99b2219` feat: confirm API key changes before activation

## T7-1 · 合并（主任务）

**动作**：在 `csa-v0.1.5-model-roles` 工作副本里，把上面两个 commit cherry-pick 过来。

```
git cherry-pick e871469 99b2219
```

有冲突就手工合，**合并原则**：
- `lib.rs` 的切换可靠性修复（去掉静默早退、revision 校验、可见日志）→ 全部保留
- `App.tsx` 的预选确认 UI → 保留，但要适配 model-roles 分支已有的界面结构，不要覆盖掉聚合/方案相关的代码

**验收**：合并后 `csa-v0.1.5-model-roles` 同时具备：聚合方案一/方案二 + 切换可靠性修复 + API Key 预选确认。

## T7-2 · 把预选确认扩展到方案切换

吕博士明确要求过：**点方案一/方案二时，也应该只是预选，点「确认」才真正切换**，不能点一下就立刻生效导致反复重启。

**动作**：参照 T7-1 合并进来的 API Key 预选确认模式，给方案（scheme-1/scheme-2）切换加同样的「预选 → 确认」交互。

**验收**：在方案一/方案二之间连点 5 次 → 零重启；点「确认切换」→ 只重启一次。

## T7-3 · 构建并交付可测版本

**动作**：
1. `npm run build`（前端，必须做，否则 UI 改动进不去）
2. 构建 launcher exe
3. 确认构建产物时间戳晚于源码改动时间

**交付**：告诉吕博士 exe 的完整路径，以及需不需要先关掉当前正在跑的那个（PID 会变）。

**注**：debug 版本即可测试，不必打正式便携包——已确认 debug 构建跑的就是新代码。

## 纪律（不变）

1. 台账 `TASKS_PROGRESS.md` 追加 T7-1/T7-2/T7-3，每完成一项立刻勾选并记「做了什么/证据/下一步」
2. 一次只做一个，做完验收再进下一个
3. 同一问题连试两次不通过 → 停手写报告，不要瞎试
4. 不许改、删、放宽 `self-test.ps1` / `verify-proxy.ps1` 的检查项

## ⚠️ 重要提醒

改完后**运行中的 Bridge 会从哪份副本启动**要确认清楚。现在 `/health` 显示 `source_path` 指向 `csa-v0.1.4-runtime-update/proxy.py`；合并完成后应该指向 `csa-v0.1.5-model-roles/proxy.py`。两份副本的 Bridge 来回换会导致聚合配置丢失，这是"感觉很怪"的一个来源。

## ⚠️ 补充（合并前必读，2026-08-09 追加）

`csa-v0.1.5-model-roles` 分支上**已经有一次早期的切换修复**：`15e0bea fix: make API key switches activate Bridge config`。

这次修复**没解决问题**（吕博士反馈仍然切不动），T0–T6 的 `e871469` 才是经过 T1 诊断后的正确修法。

**所以 cherry-pick 遇到冲突时的取舍原则：以 `e871469` 为准，覆盖 `15e0bea` 的旧写法。** 具体地，`e871469` 相比旧修复多了这几样，必须保留：
- 去掉 `bridge_running=false` 时的静默成功早退
- WSL 发行版缺失时返回错误而不是静默成功
- revision 校验同时核对当前工作副本的 `proxy.py`
- 切换前的真实上游预检 + 完整链路日志

合并完成后自检一句：`restart_bridge_after_config` 里**不应再有任何一处无日志的 `return Ok(())`**。
