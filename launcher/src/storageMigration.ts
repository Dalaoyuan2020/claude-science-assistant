export interface StorageStatusSnapshot {
  distro?: string;
  wslStoragePath?: string;
  wslStorageDrive?: string;
  wslStorageFreeGb?: number;
  wslVhdxSizeGb?: number;
  wslRootFreeGb?: number;
  settingsStorageDrive?: string;
  settingsStorageFreeGb?: number;
  storageWarning: boolean;
  storageBlocked: boolean;
  warnings: string[];
}

export type StorageRecommendationKind = "recommended" | "diagnostic" | "preventive" | "info";

export interface StorageRecommendation {
  kind: StorageRecommendationKind;
  actionLabel: string;
  title: string;
  detail: string;
  reasons: string[];
}

const displayGb = (value?: number) => typeof value === "number" ? `${value.toFixed(1)} GB` : "未检测到";

const safeLine = (value: string | undefined, fallback = "未检测到，请 Codex 只读复核") => {
  const cleaned = String(value || "").replace(/[\r\n\u0000-\u001f]+/g, " ").trim();
  return cleaned || fallback;
};

const isWindowsSystemDrive = (status: StorageStatusSnapshot) => {
  const drive = safeLine(status.wslStorageDrive, "").toUpperCase();
  const path = safeLine(status.wslStoragePath, "").toUpperCase();
  return drive === "C:" || path.startsWith("C:\\") || path.startsWith("C:/");
};

export function storageRecommendation(status: StorageStatusSnapshot): StorageRecommendation {
  const reasons: string[] = [];
  const onSystemDrive = isWindowsSystemDrive(status);
  const hostLow = typeof status.wslStorageFreeGb === "number" && status.wslStorageFreeGb < 15;
  const linuxLow = typeof status.wslRootFreeGb === "number" && status.wslRootFreeGb < 15;
  const settingsLow = typeof status.settingsStorageFreeGb === "number" && status.settingsStorageFreeGb < 10;

  if (status.storageBlocked) reasons.push("检测到只读、不可写或低于安全阻断阈值的存储状态");
  if (onSystemDrive) reasons.push("WSL 虚拟磁盘位于 Windows 系统盘 C:");
  if (hostLow) reasons.push(`VHDX 宿主盘仅剩 ${displayGb(status.wslStorageFreeGb)}`);
  if (linuxLow) reasons.push(`Linux 根分区仅剩 ${displayGb(status.wslRootFreeGb)}`);
  if (settingsLow) reasons.push(`Windows 设置盘仅剩 ${displayGb(status.settingsStorageFreeGb)}`);

  if (status.storageBlocked) {
    return {
      kind: "diagnostic",
      actionLabel: "生成诊断 Prompt",
      title: "先诊断，再决定是否迁移",
      detail: "当前状态不适合直接迁移。CSA 只生成只读体检 Prompt，不会停止服务或移动 WSL。",
      reasons,
    };
  }
  if (hostLow) {
    return {
      kind: "recommended",
      actionLabel: "推荐迁移",
      title: "建议把 WSL 迁移到空间更充足的磁盘",
      detail: "宿主盘空间已进入预警范围。请让 Codex 重新体检并生成本机迁移计划。",
      reasons,
    };
  }
  if (onSystemDrive) {
    return {
      kind: "preventive",
      actionLabel: "辅助迁移",
      title: "WSL 位于 C 盘，建议提前规划迁移",
      detail: "这是一条预防性建议，不阻断启动或增量升级。CSA 不会自行移动虚拟磁盘。",
      reasons,
    };
  }
  if (linuxLow || settingsLow || status.storageWarning) {
    return {
      kind: "diagnostic",
      actionLabel: "分析空间问题",
      title: "先判断迁移是否对症",
      detail: "Linux 根分区或 Windows 设置盘空间不足不一定能靠移动 VHDX 解决，请让 Codex 分类诊断。",
      reasons,
    };
  }
  return {
    kind: "info",
    actionLabel: "存储建议",
    title: "查看 WSL 存储位置与迁移原则",
    detail: "当前没有空间预警。仍可复制只读体检 Prompt，为大型实验提前规划存储。",
    reasons: ["当前未触发空间预警"],
  };
}

export function buildStorageMigrationPrompt(status: StorageStatusSnapshot): string {
  const recommendation = storageRecommendation(status);
  const diagnostics = status.warnings.length > 0
    ? status.warnings.slice(0, 8).map((item) => `  - ${safeLine(item)}`).join("\n")
    : "  - 无额外诊断信息";

  return [
    "请协助我评估并规划 CSA（Claude Science Assistant）所在 WSL2 发行版的存储迁移。当前打开的文件夹应是 CSA 完整便携包目录。",
    "",
    "启动器提供的提示值如下。它们只是线索，必须用本机只读命令重新核对，不能直接信任：",
    `- 触发原因：${recommendation.reasons.map((reason) => safeLine(reason)).join("；")}`,
    `- 发行版：${safeLine(status.distro)}`,
    `- WSL 虚拟磁盘/BasePath：${safeLine(status.wslStoragePath)}`,
    `- VHDX 文件大小：${displayGb(status.wslVhdxSizeGb)}`,
    `- VHDX 宿主盘剩余：${displayGb(status.wslStorageFreeGb)}`,
    `- Linux 根分区剩余：${displayGb(status.wslRootFreeGb)}`,
    `- Windows 设置盘：${safeLine(status.settingsStorageDrive)}，剩余 ${displayGb(status.settingsStorageFreeGb)}`,
    "- CSA 便携包目录：请从当前 Codex 工作区只读确认，不要使用编译机源码路径猜测",
    "- 启动器诊断摘要：",
    diagnostics,
    "",
    "先阅读并遵守当前包内的 AGENTS.md（如存在）、docs/plans/wsl-storage-migration-context-checkpoint.zh-CN.md、docs/plans/wsl-storage-migration-plan.zh-CN.md、docs/plans/wsl-storage-migration-review-result.zh-CN.md，以及 skills/bootstrap-claude-science-wsl/SKILL.md。缺少某个文件时明确报告，不要从网络下载同名脚本替代。",
    "",
    "本轮默认只允许只读检查和生成计划。不要因为我粘贴了本 Prompt 就开始迁移，也不要停止 WSL、停止服务或修改系统。",
    "",
    "硬性红线：",
    "1. 不输出、复制或记录 API Key、token、私钥、Cookie 和代理凭据。",
    "2. 不修改 VPN、代理、DNS、hosts、证书、端口 443 或用户网络工具。",
    "3. 不用资源管理器、Move-Item、robocopy 等 Windows 文件工具直接移动 ext4.vhdx。",
    "4. 不执行 wsl --unregister，不删除发行版，不修改 Lxss 注册表，不重装 Ubuntu。",
    "5. 不把 CSA 就地升级、Bridge 接管与 WSL 存储迁移合并执行。",
    "6. 不把网络盘、可移动盘、UNC、非 NTFS、reparse point 或已有非空未知目录作为目标。",
    "7. 项目文档仍标记 BUILD NO-GO 时，只能输出计划，不能执行 Move、export 或 import。",
    "",
    "第一阶段：只读体检",
    "请使用结构化、带超时的只读命令核对并脱敏汇报：",
    "1. Windows 版本、架构、重启待办，以及 wsl --status、wsl --version、wsl -l -v。",
    "2. wsl.exe --help 是否在本机明确列出 --manage 和 --move；不要只根据版本号猜测。",
    "3. 从 HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Lxss 定位目标发行版名称、GUID、Version、BasePath；确认 ext4.vhdx 存在，但不要读取其内容。",
    "4. 默认发行版、实际默认用户、id -un、PID 1/systemd 状态，以及 /etc/wsl.conf 必要键摘要；不要输出可能含本地命令或秘密的全文。",
    "5. VHDX 文件大小、源盘类型/文件系统/总量/余量；Linux 根分区 df -hT 和 df -i；/tmp 与 home 是否可写；是否只读。",
    "6. Windows 本地固定磁盘候选：盘符、NTFS、总量、余量、BitLocker 状态（若可只读获取）和目标路径是否含 reparse point。",
    "7. CSA Bridge/Claude Science 状态、端口、Bridge source_path 与 config revision，只报告状态和路径，不报告密钥。",
    "8. 是否有正在运行的实验、训练、下载或其他不能中断的 WSL 任务。不要用无边界的全盘 du 扫描。",
    "",
    "第二阶段：先判断迁移是否对症",
    "- VHDX 宿主盘空间不足或位于 C 盘时，迁移可能对症。",
    "- 如果只是 Linux 根分区余量不足而宿主盘充足，先解释清理或扩容方案；换盘本身未必增加 ext4 可用空间。",
    "- 如果只是 Windows 设置盘不足，给出设置盘清理建议，不把它伪装成 WSL 迁移问题。",
    "- 如果根分区只读、WSL 无响应、BasePath/VHDX 不一致或发行版为 WSL1，拒绝生成可执行迁移步骤，只给恢复诊断。",
    "",
    "第三阶段：推荐目标盘并计算空间",
    "只从本地固定 NTFS 卷中推荐，默认目录为 <盘符>:\\WSL\\<安全发行版名>。排除源目录子树、网络/可移动盘、非 NTFS、reparse point、受保护目录、已有发行版/VHDX 和非空未知目录。",
    "- 迁移目标盘最低余量 >= max(VHDX 实际文件大小, Linux 已用空间) * 1.2 + 15 GB",
    "- 若计划完整导出备份，备份盘最低余量 >= 估算导出大小 * 1.2 + 5 GB",
    "- 源盘执行缓冲 >= max(2 GB, VHDX 实际文件大小 * 0.05)",
    "同时检查 Windows TEMP 和 WSL swapFile 所在卷，不能把目标空间与备份空间重复计算。",
    "",
    "第四阶段：按以下格式输出并停止",
    "A. 结论：建议迁移 / 暂不建议迁移 / 必须先修复",
    "B. 触发原因与迁移是否对症",
    "C. 当前发行版与存储证据表",
    "D. 候选目标盘排序、排除理由和推荐目标目录",
    "E. 迁移能力：supported / unsupported / unknown，并列出证据与未知假设",
    "F. 备份方案、预计停机范围和最坏情况",
    "G. 拟执行阶段清单，但不要给出或运行 unregister 路径",
    "H. 迁移后验收：BasePath、VHDX、默认用户、systemd、可写性、Bridge /health、source_path、config revision、8765/8766 和空间变化",
    "I. 明确写出：本轮未修改系统、未停止服务、未执行迁移",
    "",
    "输出后必须停下等待我审阅。只有我明确回复‘同意执行上述迁移计划’，且项目 BUILD NO-GO 已正式关闭、当前机器进入已验证支持矩阵、备份和回退条件全部满足时，才可以另开执行阶段。即使进入执行阶段，也不得自动改走 export/import/unregister；Move 失败后立即停止并只读检查原发行版完整性。",
  ].join("\n");
}
