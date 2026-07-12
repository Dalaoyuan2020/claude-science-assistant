# CSA WSL 存储迁移上下文检查点

状态：2026-07-12 单一事实来源；先整理上下文，不代表迁移执行已获批准

## 1. 用户真正要的产品逻辑

1. 启动器只读检测 WSL 虚拟磁盘位置和空间状态。
2. 当宿主盘空间不足时自动推荐迁移；VHDX 位于 C 盘时，即使暂未不足也给出预防性建议。
3. 用户点击“推荐迁移 / 辅助迁移”后，弹出本机信息和一段可复制的 Codex Prompt。
4. Codex 重新做只读体检，判断“迁移是否真的能解决问题”、推荐目标盘并输出计划。
5. 启动器本身不执行 Move、export/import、unregister、注册表修改或 VHDX 文件移动。
6. 迁移执行是独立维护流程，不得和 CSA 首次安装、就地升级或 Bridge 接管混在一起。

## 2. 四个概念必须分开

| 概念 | 含义 | 当前状态 |
| --- | --- | --- |
| 存储检测 | 找到 VHDX、宿主盘余量、Linux 根分区余量、只读/inode 状态 | 已实现 |
| 迁移预警 | C 盘、低空间或不可写时在状态卡和诊断信息告警 | 已实现；C 盘专用告警仅在本地候选提交中 |
| Codex 辅助入口 | 按本机数据生成并弹出可复制 Prompt | 本地 r2 候选已实现，待最终构建与发布 |
| 实际迁移 | 停服务、备份、Move 或其他改变 WSL 状态的操作 | 未实现，仍为 `BUILD NO-GO` |

“迁移方案经过审查”不等于“启动器已经实现迁移入口”；“检测已实现”也不等于“Prompt 弹窗已实现”。此前上下文混乱正是把这三句话合并成了“迁移功能已完成”。

## 3. 代码与文档证据

| 需求 | 证据 | 结论 |
| --- | --- | --- |
| 定位 WSL BasePath / VHDX | `launcher/src-tauri/src/lib.rs` 的 Windows 存储快照与 `SystemStatus.wsl_storage_path` | 已实现 |
| 检测宿主盘与 Linux 余量 | `current_status` 计算 `wsl_storage_free_gb`、`wsl_root_free_gb` | 已实现 |
| 预警阈值 | VHDX 宿主盘或 Linux 根分区 `< 15 GB`、Windows 设置盘 `< 10 GB`、根分区余量 `< 10%`、inode `< 5%` | 已实现 |
| 阻断阈值 | 只读/不可写、VHDX 宿主盘/Linux 根分区/Windows 设置盘 `< 1 GB`、根分区或 inode 余量 `< 1%` | 已实现；只阻断危险写入/重启 |
| C 盘预防性提醒 | `wsl_on_system_drive` 警告，不阻断普通启动 | 本地提交 `589080e` 已实现，尚未推送 |
| 首页存储卡 | `launcher/src/App.tsx` 的“WSL 存储”状态项 | 已实现 |
| 迁移按钮/弹窗/复制 Prompt | `launcher/src/storageMigration.ts` 与 `launcher/src/App.tsx` | 本地 r2 候选已实现；纯前端只读，无迁移命令 |
| 迁移设计与独立审查 | `wsl-storage-migration-plan`、`review-result` | 设计存在；审查结论为 `CONDITIONAL APPROVE + BUILD NO-GO` |

## 4. 预警不能简单等同于“必须迁移”

| 检测结果 | 产品建议 |
| --- | --- |
| VHDX 在 C 盘，宿主盘空间正常 | 显示预防性“建议迁移”，允许忽略，不影响启动/升级 |
| VHDX 宿主盘空间不足 | 突出“推荐迁移”，同时提醒先停止大实验、避免继续写盘 |
| 只有 Linux 根分区空间不足，宿主盘仍充足 | 先判断清理或扩容；单纯换盘未必增加 ext4 可用空间 |
| Windows 设置盘空间不足 | 建议清理设置盘；这不是 WSL 迁移能直接解决的问题 |
| 根分区只读、WSL 无响应或注册信息不一致 | 只生成诊断，禁止把迁移当修复手段 |

## 5. 下一版最小可交付范围

可以实现，而且难点不在弹窗本身，而在不能向用户生成错误或破坏性的迁移指令。最小版本只做以下内容：

- 在“WSL 存储”卡或诊断区显示“辅助迁移”。
- 弹窗展示发行版、VHDX 位置、宿主盘余量、Linux 余量和触发原因。
- 根据当前状态生成 `docs/prompts/csa-wsl-storage-migration-codex-prompt.zh-CN.md` 的本机化文本。
- 提供“复制 Prompt”和关闭按钮，不调用任何迁移命令。
- 当根只读、WSL 无响应、路径未知时，把 Prompt 降级成“只读诊断 Prompt”。
- 单元测试证明点击入口不会调用 Tauri 的服务启停、配置写入或任何 WSL 迁移命令。

## 6. 发布状态

- 当前公开 v0.1.3 首发 ZIP 不含该入口；待以独立 r2 资产发布，不能静默覆盖首发 ZIP。
- 本地分支 `codex/csa-v0.1.3-release` 比远端多提交 `589080e`，包含 C 盘预警和 Bridge 修复，但尚未推送。
- 最新本地候选包为 `dist/release-v0.1.3-r2-final-20260712-3/`；尚未完成该目录的最终包级复验，也不应上传。
- Prompt 弹窗完成、测试和重新打包前，不得宣称 Release 已支持“交给 Codex 迁移”。

## 7. 当前决策

本轮先固定上下文和 Prompt。后续 Build 可以开放“只读检测 + 推荐 + Prompt 弹窗”，但实际迁移仍受 `AGENTS.md` 和独立审查的 `BUILD NO-GO` 约束。关闭 309/VM 能力与失败恢复测试门槛之前，不把 Codex Prompt 写成无条件执行迁移的授权书。
