# CSA WSL 存储迁移方案：外部严格审查 Prompt

用途：把下面整段 Prompt 交给另一个 Agent。

要求：审查者只做只读Review，不实施、不迁移、不修改代码。
审查结果目标路径：`docs/plans/wsl-storage-migration-review-result.zh-CN.md`

---

你现在是 CSA（Claude Science Assistant）WSL 存储迁移功能的独立安全审查者。请采用第一性原理、对抗性审查、故障注入和数据丢失威胁建模，对迁移方案做严格 Review。

本轮只允许只读检查和撰写审查报告。禁止执行任何迁移、停止WSL、停止服务、导出、导入、Move、unregister、删除、改注册表、修改系统设置或构建代码。不要实现修复。

## 一、必须阅读的文件

首先完整阅读：

1. `docs/plans/wsl-storage-migration-plan.zh-CN.md`
2. `AGENTS.md`
3. `SECURITY.md`
4. `README.md`中的存储、升级和安全章节
5. `launcher/src-tauri/src/lib.rs`
6. `launcher/src/App.tsx`
7. `launcher/src/App.css`
8. `scripts/status-probe.ps1`
9. `scripts/start-claude-science-wsl.ps1`
10. `scripts/start-claude-science-wsl.sh`
11. `skills/bootstrap-claude-science-wsl/SKILL.md`
12. `skills/bootstrap-claude-science-wsl/scripts/inspect-windows.ps1`
13. `skills/bootstrap-claude-science-wsl/scripts/inspect-wsl.sh`
14. `skills/bootstrap-claude-science-wsl/references/rollback.md`
15. `docs/v0.1-current-pc-verification.zh-CN.md`

确认文件真实存在。不要根据本Prompt假定代码已经实现迁移；当前方案应当仍是Design Only。

## 二、审查方法

不要只评价文案是否清晰。必须验证方案在真实Windows/WSL语义下是否成立，并主动寻找会导致以下结果的路径：

- 用户整个WSL发行版或home目录丢失。
- Move失败后错误进入unregister路径。
- 备份看似成功但无法恢复。
- 默认用户变为root。
- systemd、Bridge、Claude Science或配置权限失效。
- 迁移了错误的发行版、Docker发行版或另一个用户的数据。
- 目标路径被junction/symlink/路径注入劫持。
- 迁移过程中窗口冻结、命令无限等待或用户误以为完成。
- 计划生成后环境变化，但旧计划仍被执行。
- API Key、token、路径secret或用户文件内容进入日志/计划/诊断包。
- C盘几乎写满时，备份、临时文件或WSL命令再次写爆C盘。
- 目标盘断开、BitLocker锁定、空间虚报或文件系统不兼容。
- Windows 10/11、Store WSL/系统内置WSL、Ubuntu-22.04/24.04之间行为不同。
- 迁移成功但旧VHDX仍占C盘，或两个副本同时被误认为活动实例。

使用“假设不成立会怎样”的方式逐条挑战方案。不要因当前开发机支持 `wsl --manage --move` 就假设所有目标机器支持。

## 三、必须核实的WSL事实

优先查阅当前目标机器的 `wsl.exe --help` / `wsl --version` 和微软官方文档。涉及WSL命令语义时，只接受微软官方来源作为外部事实依据，并在报告中附URL和访问日期。

至少核实：

1. `wsl --manage <Distro> --move <Location>`的可用版本、停机要求、路径限制、失败语义和保留内容。
2. Move是否保留发行版名、GUID、默认用户、WSL版本、默认发行版状态和稀疏VHD属性。
3. `wsl --export`支持的格式、输出空间和失败后文件可信度。
4. `wsl --unregister`的破坏性边界。
5. `wsl --import`与`--import-in-place`对默认用户、发行版标识、VHD格式和目录结构的影响。
6. Store安装发行版、手动import发行版、Windows 10与Windows 11的差异。
7. 是否需要管理员权限；哪些目标目录/策略可能触发权限问题。
8. `wsl --shutdown`与精确`--terminate`的取舍。

如果官方资料没有明确保证，必须标为“未知/不可依赖”，不能用经验猜测补齐。

## 四、必须审查的设计面

### A. 产品边界

- “辅助迁移”是否会被用户误解为无风险一键迁移。
- 是否清楚说明移动的是整个发行版，而不是仅CSA。
- 是否把普通启动、Provider切换和迁移状态隔离。
- UI是否有足够明确的停机、备份、目标盘和回滚信息。

### B. 状态机与恢复

- 每个状态是否可持久化、可重复进入、可从崩溃恢复。
- 每个破坏性阶段前是否有独立确认。
- 应用、Codex或Windows中断后，下一次如何判断真实状态。
- 是否存在“命令成功但状态文件没写”或反向情况。
- 迁移锁如何防止两个Agent/两个启动器并发执行。

### C. 路径与命令安全

- Windows路径、发行版名、用户名、卷标是否都按不可信输入处理。
- 是否避免PowerShell/Bash字符串拼接和命令注入。
- 是否解析最终绝对路径并检查junction/symlink/reparse point。
- 是否能证明删除/Move/备份目标都在预期目录。
- 是否避免直接修改Lxss注册表。

### D. 空间估算

- `VHDX逻辑大小`与物理占用、稀疏文件、导出文件大小之间是否混淆。
- 1.2倍或+15GB规则是否足够，最坏情况是什么。
- 同盘目标+备份是否重复计算空间。
- C盘低空间时Windows TEMP、WSL和日志是否仍可能写入C盘。
- 是否需要单独检查inode、目标盘文件系统、配额、BitLocker和磁盘健康。

### E. 备份与回滚

- “备份存在且大小合理”是否足以称为可恢复。
- 如何验证备份而不破坏原环境。
- 快速无备份模式是否应存在。
- 兼容路径是否在任何情况下可能自动unregister。
- Move成功但应用失败时，是否错误地把应用故障当磁盘迁移失败。
- 反向Move/恢复import是否有足够源盘空间。

### F. CSA后验检查

- Bridge `source_path`、config revision、9876、8765/8766是否足够。
- 是否验证默认用户、systemd、home、`/tmp`、配置0600和managed binary。
- 是否避免默认发起付费模型请求。
- 是否能识别旧VHDX残留与真实活动VHDX。

### G. 适配性

- Windows 10 19045 + Ubuntu-22.04（309场景）。
- Windows 11 + Ubuntu-24.04。
- systemd开启/关闭。
- Store WSL版本和旧版WSL。
- C/D/E/H盘、中文/空格/长路径。
- 多发行版、默认发行版、Docker发行版。

### H. 可测试性

- 是否能在不真实销毁用户发行版的情况下测试大部分逻辑。
- 哪些测试必须使用临时测试发行版/VM。
- 是否覆盖超时、断电、进程终止、空间变化、目标盘移除和恢复失败。
- 测试结束是否能证明没有影响用户真实发行版。

## 五、严重级别

所有发现按以下等级排序：

- `P0`：可能导致数据丢失、迁移错误发行版、秘密泄漏、不可恢复破坏；Build前必须解决。
- `P1`：高概率导致迁移失败、错误成功提示、无法回滚或跨环境严重不兼容；Build前必须解决。
- `P2`：影响可靠性、诊断、可维护性或用户理解；应在首版实现前解决或明确接受。
- `P3`：文案、交互和低风险改进。

不要为了显得严格而虚构问题。每条发现必须包含：

1. 严重级别和简短标题。
2. 方案章节或现有代码文件/行号证据。
3. 触发条件。
4. 最坏后果。
5. 为什么现有保护不足。
6. 可验证的修改建议。
7. 对应测试或验收方法。

## 六、必须回答的问题

1. v0.1.4只做“生成方案+Codex执行”是否足够安全？
2. 是否应该完全删除“快速无完整备份”模式？
3. 官方Move前是否必须导出备份；如果不是，最低安全门槛是什么？
4. 兼容Export-Import路径是否应推迟到v0.2或更晚？
5. 目标盘应仅限NTFS，还是可以支持ReFS；依据是什么？
6. 计划指纹应包含哪些字段，如何避免TOCTOU？
7. 迁移锁放在哪里，如何处理两个Windows用户或两个Agent？
8. 什么情况下必须拒绝迁移，而不是继续给出警告？
9. 迁移成功的最小充分证据是什么？
10. 哪些未知事项需要在309或临时VM上做实验才能决定？

## 七、输出格式

把完整报告写入：

`docs/plans/wsl-storage-migration-review-result.zh-CN.md`

报告必须按以下结构：

```markdown
# CSA WSL 存储迁移方案独立审查报告

## 1. 审查结论
- Verdict: APPROVE / CONDITIONAL APPROVE / REJECT
- Build Gate: GO / NO-GO
- 一句话原因

## 2. 审查范围与证据
- 已读文件
- 使用的官方资料
- 未能验证的事实

## 3. P0发现

## 4. P1发现

## 5. P2/P3发现

## 6. 需求与风险追踪表
| 方案要求 | 风险 | 当前保护 | 缺口 | 建议 |

## 7. 迁移方式评审
- 官方Move
- Export-Import
- Import-in-place

## 8. 状态机与恢复评审

## 9. 路径、权限与命令安全评审

## 10. 空间、备份与回滚评审

## 11. 测试矩阵缺口

## 12. 对10个必答问题的逐项回答

## 13. 建议修改稿
- 对方案的具体替换/新增文本

## 14. Build前清单
- [ ] 所有P0关闭
- [ ] 所有P1关闭或被用户明确接受
- [ ] 官方WSL语义有引用
- [ ] 恢复路径可演练
- [ ] 309/VM实验项明确

## 15. 最终建议
```

结论规则：

- 任一未解决P0：`REJECT + NO-GO`。
- 任一会导致数据丢失或自动unregister的P1：至少`CONDITIONAL APPROVE + NO-GO`。
- 不得因方案“总体方向正确”而省略具体发现。
- 如果没有发现问题，必须说明仍然存在的残余风险和未执行测试。

## 八、审查完成后的行为

只写审查报告，不修改迁移方案、不修改代码、不构建、不执行任何WSL命令中的变更操作。完成后向用户简要报告：Verdict、P0数量、P1数量、最关键的三个修改点，以及报告文件路径。

---
