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
    "你是 CSA 本地存储迁移诊断 Agent。目标是做可行性扫描和迁移计划，不在本轮执行迁移。",
    "",
    "当前面板线索：",
    `- 触发原因：${recommendation.reasons.map((reason) => safeLine(reason)).join("；") || "用户主动检查"}`,
    `- WSL 发行版：${safeLine(status.distro)}`,
    `- WSL VHDX/BasePath：${safeLine(status.wslStoragePath)}`,
    `- VHDX 文件大小：${displayGb(status.wslVhdxSizeGb)}`,
    `- VHDX 宿主盘剩余：${displayGb(status.wslStorageFreeGb)}`,
    `- Linux 根分区剩余：${displayGb(status.wslRootFreeGb)}`,
    `- Windows 设置盘：${safeLine(status.settingsStorageDrive)}，剩余 ${displayGb(status.settingsStorageFreeGb)}`,
    "- 启动器诊断摘录：",
    diagnostics,
    "",
    "本轮只允许只读扫描：",
    "1. 检查 Windows 固定磁盘容量、文件系统、剩余空间和候选目标目录。",
    "2. 检查 WSL 发行版、BasePath、ext4.vhdx 是否存在、VHDX 大小、wsl -l -v、wsl --status。",
    "3. 检查 Linux 根分区 df -hT / df -i、/tmp 和 /home 可写性、是否有正在运行的训练/下载/服务。",
    "4. 检查 CSA 项目路径、Bridge/Claude Science 端口和运行状态，只汇报状态，不读取密钥。",
    "   Bridge source_path 和 config revision 只作为诊断记录，不作为迁移阻断条件。",
    "5. 如果需要扫 C 盘占用，只做只读容量归类，不删除、不移动、不清理。",
    "",
    "硬边界：不要停止 WSL/Ubuntu，不要移动 VHDX，不要执行 export/import/unregister，不要修改注册表、代理、DNS、hosts、证书、端口、防火墙或系统服务。不要输出 API Key、token、cookie、私钥、完整 .env 或浏览器数据。",
    "BUILD NO-GO：当前项目只允许输出迁移计划，不能执行 Move/export/import；必须等待用户明确批准并正式关闭该标记。",
    "严禁执行 wsl --unregister，不删除发行版，不修改 Lxss 注册表。",
    "",
    "请输出到结果区的结构：",
    "A. 结论：建议迁移 / 暂不迁移 / 需要先修复。",
    "B. 证据表：当前 VHDX、C 盘、候选目标盘、Linux 根分区、运行中任务。",
    "C. 候选目标盘排序和排除理由，默认目标目录格式为 <盘符>:\\WSL\\<发行版名>。",
    "D. 推荐迁移方案：优先说明 wsl --manage --move 是否本机支持；不支持时给 export/import 备选计划。",
    "E. 执行前清单：需要用户关闭哪些任务、何时 wsl --shutdown、备份和回退方案。",
    "F. 执行命令草案：只给草案，不要运行；危险命令必须标注“等待用户显式确认”。",
    "G. 验收清单：BasePath、VHDX、默认用户、systemd、可写性、Bridge /health、端口、空间变化。",
    "",
    "最后明确写出：本轮未修改系统、未停止服务、未执行迁移，只完成扫描与计划。",
  ].join("\n");

}
