import type { StorageStatusSnapshot } from "./storageMigration";

export type AgentTaskId = "dataset" | "environment" | "vm" | "migration";

export interface AgentTaskCard {
  id: AgentTaskId;
  title: string;
  badge: string;
  summary: string;
  checks: string[];
}

export const agentTaskCards: AgentTaskCard[] = [
  {
    id: "dataset",
    title: "数据集下载",
    badge: "Data",
    summary: "沙盒内下载失败时，让本机 Agent 检查 URL、磁盘、代理和断点续传方案。",
    checks: ["URL/鉴权", "目标磁盘", "断点续传", "校验哈希"],
  },
  {
    id: "environment",
    title: "环境配置",
    badge: "Env",
    summary: "让 Agent 诊断 Python、Node、Docker、Conda、uv、CUDA 与依赖安装卡点。",
    checks: ["依赖探测", "安装计划", "冲突版本", "可复现命令"],
  },
  {
    id: "vm",
    title: "虚拟机与硬件",
    badge: "VM",
    summary: "检查 WSL/虚拟机、SSH 端口、GPU、显存、驱动和运行队列是否可用。",
    checks: ["SSH/端口", "GPU/显存", "WSL/Docker", "服务状态"],
  },
  {
    id: "migration",
    title: "迁移扫描",
    badge: "Move",
    summary: "先扫描 C 盘、WSL VHDX 和候选目标盘，再生成迁移计划和回退清单。",
    checks: ["容量扫描", "目标盘筛选", "停机边界", "回退方案"],
  },
];

const displayGb = (value?: number) => typeof value === "number" ? `${value.toFixed(1)} GB` : "unknown";

const clean = (value?: string) => String(value || "").replace(/[\r\n\u0000-\u001f]+/g, " ").trim() || "unknown";

export function buildAgentTaskPrompt(
  taskId: AgentTaskId,
  note: string,
  status: StorageStatusSnapshot,
  migrationPrompt: string,
) {
  const common = [
    "你是 CSA 外部 Agent 协作工程师。当前任务来自 CSA 面板。",
    "默认只读：不要删除、移动、安装、上传、改系统代理、改 DNS、改 hosts、改证书、停止 WSL 或重启服务。",
    "不要输出 API Key、token、cookie、私钥、完整 .env、完整对话记录或浏览器数据。",
    "先给出诊断证据、可执行计划、需要用户确认的命令，再等待人工确认。",
    "",
    "当前机器线索：",
    `- WSL 发行版：${clean(status.distro)}`,
    `- WSL VHDX/BasePath：${clean(status.wslStoragePath)}`,
    `- VHDX 大小：${displayGb(status.wslVhdxSizeGb)}`,
    `- VHDX 宿主盘剩余：${displayGb(status.wslStorageFreeGb)}`,
    `- Linux 根分区剩余：${displayGb(status.wslRootFreeGb)}`,
    `- Windows 设置盘：${clean(status.settingsStorageDrive)} / ${displayGb(status.settingsStorageFreeGb)}`,
    "",
    "用户补充：",
    note.trim() || "无；请先基于本机只读探测提出下一步需要用户补充的信息。",
    "",
  ];

  if (taskId === "dataset") {
    return [
      ...common,
      "任务：数据集下载卡点诊断。",
      "目标：判断沙盒内下载失败时，能否改由本机/宿主机下载，然后把数据集放到项目可访问的位置。",
      "请只读检查：磁盘剩余、下载工具可用性、代理环境变量、目标目录是否存在、是否已有部分文件、是否需要断点续传。",
      "输出：推荐下载命令、目标目录、校验方式、失败补救方案。不要真实下载，除非用户明确授权。",
    ].join("\n");
  }

  if (taskId === "environment") {
    return [
      ...common,
      "任务：环境配置与安装卡点诊断。",
      "目标：判断依赖安装失败是 Python/Node/Docker/Conda/uv/CUDA/权限/网络/磁盘中的哪一类问题。",
      "请只读检查：版本、虚拟环境、锁文件、requirements/pyproject/package.json、CUDA/nvidia-smi、Docker 状态。",
      "输出：最小可复现安装计划、可回退命令、需要外部 Agent 执行的任务拆分。不要直接安装。",
    ].join("\n");
  }

  if (taskId === "vm") {
    return [
      ...common,
      "任务：虚拟机、SSH、端口与硬件可用性检测。",
      "目标：判断虚拟机/WSL/Docker 是否可控，SSH 端口是否可连，GPU/显存/驱动是否满足任务。",
      "请只读检查：wsl -l -v、端口监听、ssh 可达性、nvidia-smi、Docker/WSL 服务状态。",
      "输出：连接状态表、失败原因、最小修复计划。不要停止虚拟机或修改防火墙。",
    ].join("\n");
  }

  return [
    ...common,
    "任务：WSL/磁盘迁移扫描与计划生成。",
    "目标：先用只读扫描找到 C 盘/WSL VHDX/候选目标盘的容量证据，再生成迁移计划。",
    "请遵守下面的迁移专用 Prompt。当前轮只允许扫描与计划，不允许执行迁移。",
    "",
    migrationPrompt,
  ].join("\n");
}
