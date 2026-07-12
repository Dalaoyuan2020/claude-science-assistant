# CSA WSL 存储辅助迁移 Codex Prompt

用途：由 CSA 启动器填入本机检测值后展示给用户复制。当前版本先让 Codex 只读体检、判断问题类型并生成计划；粘贴本 Prompt 不等于授权执行迁移。

## 可直接复制的 Prompt

```text
请协助我评估并规划 CSA 所在 WSL2 发行版的存储迁移。你现在位于 CSA 完整便携包目录。

启动器提供的提示值如下；它们只是线索，必须用本机只读命令重新核对，不能直接信任：
- 触发原因：{{TRIGGER_REASON}}
- 发行版：{{DISTRO_NAME}}
- WSL 虚拟磁盘/BasePath：{{WSL_STORAGE_PATH}}
- VHDX 大小：{{VHDX_SIZE_GB}} GB
- VHDX 宿主盘剩余：{{HOST_FREE_GB}} GB
- Linux 根分区剩余：{{LINUX_ROOT_FREE_GB}} GB
- CSA 便携包目录：{{CSA_PACKAGE_PATH}}

先阅读并遵守：
1. AGENTS.md
2. docs/plans/wsl-storage-migration-context-checkpoint.zh-CN.md
3. docs/plans/wsl-storage-migration-plan.zh-CN.md
4. docs/plans/wsl-storage-migration-review-result.zh-CN.md
5. skills/bootstrap-claude-science-wsl/SKILL.md

本轮默认只允许只读检查和生成计划。不要因为我粘贴了本 Prompt 就开始迁移，也不要停止 WSL、停止服务或修改系统。

硬性红线：
- 不输出、复制或记录 API Key、token、私钥和代理凭据。
- 不修改 VPN、代理、DNS、hosts、证书、端口 443 或用户网络工具。
- 不用资源管理器、Move-Item、robocopy 等 Windows 文件工具直接移动 ext4.vhdx。
- 不执行 wsl --unregister，不删除发行版，不修改 Lxss 注册表，不重装 Ubuntu。
- 不把 CSA 就地升级、Bridge 接管与 WSL 存储迁移合并执行。
- 不把网络盘、可移动盘、UNC、非 NTFS、reparse point 或现有非空目录作为目标。
- 项目文档仍标记 BUILD NO-GO 时，只能输出计划，不能执行 Move/export/import。

第一阶段：只读体检
请使用结构化、带超时的只读命令核对并脱敏汇报：
1. Windows 版本、架构、重启待办，以及 wsl --status、wsl --version、wsl -l -v。
2. wsl.exe --help 是否在本机明确列出 --manage 和 --move；不要只根据版本号猜测。
3. 从 HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss 定位目标发行版的名称、GUID、Version、BasePath；确认 ext4.vhdx 存在，但不要读取其内容。
4. 默认发行版、实际默认用户、id -un、PID 1/systemd 状态，以及 /etc/wsl.conf 的必要键摘要；不要输出可能含本地命令或秘密的全文。
5. VHDX 文件大小、源盘类型/文件系统/总量/余量；Linux 根分区 df -hT 和 df -i；/tmp 与 home 是否可写；是否只读。
6. Windows 本地固定磁盘候选：盘符、NTFS、总量、余量、BitLocker 状态（若可只读获取）和目标路径是否含 reparse point。
7. CSA Bridge/Claude Science 当前状态、端口、Bridge source_path 与 config revision，只报告状态和路径，不报告密钥。
8. 是否有正在运行的实验、训练、下载或其他不能中断的 WSL 任务。不要用无边界的全盘 du 扫描。

第二阶段：先判断迁移是否对症
- 如果 VHDX 宿主盘空间不足或它位于 C 盘，迁移可能对症。
- 如果只是 Linux 根分区余量不足而宿主盘充足，先解释清理或扩容方案；换盘本身未必增加 ext4 可用空间。
- 如果只是 Windows 设置盘不足，给出设置盘清理建议，不把它伪装成 WSL 迁移问题。
- 如果根分区只读、WSL 无响应、BasePath/VHDX 不一致或发行版为 WSL1，拒绝生成可执行迁移步骤，只给恢复诊断。

第三阶段：推荐目标盘并计算空间
只从本地固定 NTFS 卷中推荐，默认目录为 <盘符>:\WSL\<安全发行版名>。逐项排除源目录子树、网络/可移动盘、非 NTFS、reparse point、受保护目录、已有发行版/VHDX 和非空未知目录。

使用以下保守门槛并列出计算过程：
- 迁移目标盘最低余量 >= max(VHDX 实际文件大小, Linux 已用空间) * 1.2 + 15 GB
- 若计划完整导出备份，备份盘最低余量 >= 估算导出大小 * 1.2 + 5 GB
- 源盘执行缓冲 >= max(2 GB, VHDX 实际文件大小 * 0.05)
同时检查 Windows TEMP 和 WSL swapFile 所在卷，不能把目标空间与备份空间重复计算。

第四阶段：按以下格式输出并停止
A. 结论：建议迁移 / 暂不建议迁移 / 必须先修复
B. 触发原因与“迁移是否对症”
C. 当前发行版与存储证据表
D. 候选目标盘排序、排除理由和推荐目标目录
E. 迁移能力：supported / unsupported / unknown，并列出证据来源与未知假设
F. 备份方案、预计停机范围和最坏情况
G. 拟执行阶段清单，但不要给出或运行 unregister 路径
H. 迁移后验收清单：BasePath、VHDX、默认用户、systemd、可写性、Bridge /health、source_path、config revision、8765/8766 和空间变化
I. 明确写出：本轮未修改系统、未停止服务、未执行迁移

输出后必须停下，等待我审阅。只有我明确回复“同意执行上述迁移计划”，且项目的 BUILD NO-GO 已被正式关闭、当前机器进入已验证支持矩阵、备份和回退条件全部满足时，才可以另开执行阶段。即使进入执行阶段，也不得自动改走 export/import/unregister；Move 失败后立即停止并只读检查原发行版完整性。
```

## 启动器填充值规则

- 未知值统一填“未检测到，请 Codex 只读复核”，不要省略字段。
- `TRIGGER_REASON` 必须区分“C 盘预防性建议”“VHDX 宿主盘空间不足”“Linux 根分区空间不足”“只读/不可写”等，不得一律写“空间不足”。
- Prompt 中不得嵌入 API Key、完整环境变量、配置文件内容或用户主目录文件清单。
- 路径保留原始大小写和空格；展示时使用纯文本，不拼成可执行 shell 字符串。
