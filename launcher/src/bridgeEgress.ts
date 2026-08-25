export type BridgeEgressLayerState = "passed" | "failed" | "skipped";

export interface BridgeEgressLayer {
  state: BridgeEgressLayerState;
  code: string;
  httpStatus?: number;
  durationMs: number;
  detail?: string;
}

export interface BridgeEgressCandidate {
  address: string;
  source?: string;
  processName?: string;
  tcp?: BridgeEgressLayer;
  upstream?: BridgeEgressLayer;
  recommended?: boolean;
  reason?: string;
}

export interface BridgeEgressReport {
  operation: "bridge_egress";
  ok: boolean;
  code: string;
  conclusion: string;
  billableRequestSent: boolean;
  model?: string;
  outboundProxyConfigured: boolean;
  outboundProxyUrl?: string;
  upstreamBaseUrl?: string;
  health: BridgeEgressLayer;
  proxy: BridgeEgressLayer;
  models: BridgeEgressLayer;
  request: BridgeEgressLayer;
  direct: BridgeEgressLayer;
  candidates?: BridgeEgressCandidate[];
  suggestedAction: string;
  warnings: string[];
}

export interface BridgeEgressApplyResult {
  operation: "bridge_egress_apply";
  ok: boolean;
  code: string;
  backupPath: string;
  beforeOutboundProxyUrl: string;
  afterOutboundProxyUrl: string;
  afterProbe: BridgeEgressReport;
}

const safeLine = (value: unknown, fallback = "未提供") => {
  const cleaned = String(value ?? "")
    .replace(/[\r\n\u0000-\u001f\u007f]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  return (cleaned || fallback).slice(0, 500);
};

const safeCode = (value: unknown) => {
  const code = String(value ?? "").trim();
  return /^work\.bridge_egress(?:\.[a-z0-9_]+)+$/.test(code)
    ? code
    : "work.bridge_egress.probe_invalid";
};

const sanitizedUrl = (value: string | undefined) => {
  if (!value) return "未配置";
  try {
    const parsed = new URL(value);
    if (!parsed.protocol || !parsed.hostname) return "无效地址（已脱敏）";
    return `${parsed.protocol}//${parsed.host}`.slice(0, 220);
  } catch {
    return "无效地址（已脱敏）";
  }
};

const candidateIsDirect = (candidate: BridgeEgressCandidate) => {
  const source = String(candidate.source ?? "").toLowerCase();
  const address = String(candidate.address ?? "").trim().toLowerCase();
  return source.includes("direct") || address === "" || address === "direct";
};

export const displayBridgeEgressAddress = (value: string | undefined) => sanitizedUrl(value);

export const displayBridgeEgressCandidateAddress = (candidate: BridgeEgressCandidate) => (
  candidateIsDirect(candidate) ? "直连（outbound_proxy_url 置空）" : sanitizedUrl(candidate.address)
);

const candidateIsSystemProxy = (candidate: BridgeEgressCandidate) => (
  String(candidate.source ?? "").toLowerCase().includes("system")
);

const candidateState = (layer: BridgeEgressLayer | undefined) => layer?.state ?? "skipped";

const sortedCandidates = (candidates: BridgeEgressCandidate[] | undefined) => (
  (candidates ?? [])
    .map((candidate, index) => ({ candidate, index }))
    .sort((left, right) => {
      const leftReachable = candidateState(left.candidate.upstream) === "passed" ? 1 : 0;
      const rightReachable = candidateState(right.candidate.upstream) === "passed" ? 1 : 0;
      if (leftReachable !== rightReachable) return rightReachable - leftReachable;
      const leftSystem = candidateIsSystemProxy(left.candidate) ? 1 : 0;
      const rightSystem = candidateIsSystemProxy(right.candidate) ? 1 : 0;
      if (leftSystem !== rightSystem) return rightSystem - leftSystem;
      const leftDirect = candidateIsDirect(left.candidate) ? 1 : 0;
      const rightDirect = candidateIsDirect(right.candidate) ? 1 : 0;
      if (leftDirect !== rightDirect) return leftDirect - rightDirect;
      return left.index - right.index;
    })
    .map(({ candidate }) => candidate)
);

const markdownCell = (value: unknown, fallback = "未提供") => safeLine(value, fallback).replace(/\|/g, "\\|");

const candidateLayerCell = (layer: BridgeEgressLayer | undefined) => {
  if (!layer) return "未探测";
  const duration = Number.isFinite(layer.durationMs) ? ` / ${Math.max(0, Math.round(layer.durationMs))} ms` : "";
  return `${markdownCell(layer.state)} / ${safeCode(layer.code)}${duration}`;
};

const candidateTable = (candidates: BridgeEgressCandidate[]) => {
  if (candidates.length === 0) return ["- 本次报告未返回候选；不要猜测或自动改配置。"];
  return [
    "| 顺位 | 地址 | 来源 / 进程 | TCP | 上游 origin 传输层 | 推荐 | 理由 |",
    "|---:|---|---|---|---|---|---|",
    ...candidates.map((candidate, index) => {
      const address = candidateIsDirect(candidate) ? "直连（空值）" : sanitizedUrl(candidate.address);
      const source = `${markdownCell(candidate.source, "未知来源")}${candidate.processName ? ` / ${markdownCell(candidate.processName)}` : ""}`;
      return `| ${index + 1} | ${markdownCell(address)} | ${source} | ${candidateLayerCell(candidate.tcp)} | ${candidateLayerCell(candidate.upstream)} | ${candidate.recommended ? "是" : "否"} | ${markdownCell(candidate.reason, "未提供")} |`;
    }),
  ];
};

const layerLine = (label: string, layer: BridgeEgressLayer) => {
  const http = typeof layer.httpStatus === "number" ? `，HTTP ${layer.httpStatus}` : "";
  const duration = Number.isFinite(layer.durationMs) ? `，${Math.max(0, Math.round(layer.durationMs))} ms` : "";
  const detail = layer.detail ? `，${safeLine(layer.detail)}` : "";
  return `- ${label}：${layer.state}，${safeCode(layer.code)}${http}${duration}${detail}`;
};

export function buildBridgeEgressRepairPrompt(report: BridgeEgressReport): string {
  const proxy = sanitizedUrl(report.outboundProxyUrl);
  const upstream = sanitizedUrl(report.upstreamBaseUrl);
  const proxyDead = safeCode(report.code) === "work.bridge_egress.proxy_dead";
  const candidates = sortedCandidates(report.candidates);
  const recommended = candidates.find((candidate) => candidate.recommended);
  const recommendedAddress = recommended
    ? (candidateIsDirect(recommended) ? "" : sanitizedUrl(recommended.address))
    : undefined;
  const recommendedLabel = recommended
    ? (candidateIsDirect(recommended) ? "直连（outbound_proxy_url 置空）" : recommendedAddress)
    : "无";
  const directWarning = recommended && candidateIsDirect(recommended)
    ? `- 直连警告：直连可能到不了 OpenAI / Anthropic。只有用户确认自己使用的上游全部是国内服务，并逐个通过真实请求证明可直连时，才可把直连视为够用；当前候选探测只证明 ${upstream} 的 origin 传输层，不证明完整 base URL 路径或模型请求。`
    : "- 当前推荐不是直连；候选探测只证明当前上游 origin 传输层，完整 base URL 路径仍须由应用后的真实请求验证。";
  const partialBody = recommendedAddress === undefined
    ? undefined
    : JSON.stringify({ outbound_proxy_url: recommendedAddress });
  const warnings = report.warnings.length > 0
    ? report.warnings.slice(0, 8).map((warning) => `- ${safeLine(warning)}`).join("\n")
    : "- 无额外警告";

  return [
    "请协助我只读复核并修复 CSA Bridge → 上游模型 API 的真实出口。当前目录应是 CSA V0.1.7 完整便携包或其源码工作树。",
    "",
    "启动器观测值（均须用本机只读命令复核；不得把它们当作授权直接修改）：",
    `- 顶层判定：${safeCode(report.code)}`,
    `- 结论：${safeLine(report.conclusion)}`,
    `- 本次是否已发真实请求：${report.billableRequestSent ? "是；最多 1 次，max_tokens=1" : "否"}`,
    `- Bridge-facing 模型：${safeLine(report.model, "未选择")}`,
    `- outbound proxy：${report.outboundProxyConfigured ? proxy : "未配置"}`,
    `- 上游 base URL：${upstream}`,
    layerLine("1. Bridge /health", report.health),
    layerLine("2. outbound proxy TCP", report.proxy),
    layerLine("3. Bridge /v1/models", report.models),
    layerLine("4. 最小 /v1/messages", report.request),
    layerLine("5. 不经 outbound proxy 的直连对照", report.direct),
    "- 额外警告：",
    warnings,
    "",
    "出口候选（按‘当前上游 origin 传输层可达 → Windows 系统代理一致 → 直连最后’排序）：",
    ...candidateTable(candidates),
    "",
    "推荐项：",
    recommended
      ? `- ${recommendedLabel}；为什么：${safeLine(recommended.reason, "该候选按探测结果与来源优先级排在首位。")}`
      : "- 本次没有可验证的推荐项；保持只读，不要猜一个端口。",
    directWarning,
    "",
    "建议动作：",
    proxyDead
      ? "- 已命中 work.bridge_egress.proxy_dead。请复核候选表的 TCP 与当前上游结果，并只采用已标记的推荐项；不要仅因旧端口失联就默认改成直连。"
      : `- ${safeLine(report.suggestedAction)}`,
    "- 先展示拟修改字段、修改前后的脱敏值、备份位置、局部 POST、验证和回退方式，然后停止等待我明确批准。",
    "",
    "最小改动（仅在用户明确批准后执行；不是整份覆写）：",
    ...(partialBody ? [
      `- 优先调用启动器受管 command：apply_bridge_egress_fix(candidate_url=${JSON.stringify(recommendedAddress)})。不要把它改写成 wsl.exe bash -lc。`,
      "- 受管 command 首先把 config.json 备份为 config.json.bak-<yyyymmdd-HHMMSS>，权限固定为 0600。",
      "- 它在 PID、源码哈希、监听者和受管配置目录复核通过后，内部执行单字段管理请求；不要用裸 curl 绕过身份校验或本地控制令牌。",
      `- POST body 只含一个键：${partialBody}`,
      "- 若 /health 或 egress 验证失败，受管 command 从 0600 备份中只提取旧 outbound_proxy_url，经同一个单字段管理请求恢复并读回 /health；备份保留作证据，不整份覆盖并发配置；报 work.bridge_egress.apply_rolled_back。",
    ] : ["- 没有推荐候选，因此不生成写命令。先补齐只读证据。"]),
    "",
    "硬性红线：",
    "1. 本轮先只读复核；未经我明确批准，不修改 outbound_proxy_url 或任何配置。",
    "2. 不读取、输出、复制或记录 API Key、token、私钥、Cookie、请求头、响应正文或代理凭据。",
    "3. 不输出或记录 /api/config 响应正文；受管 command 只做有界读取并解析 ok 标志，最终只用 /health 的脱敏字段验证。",
    "4. 不修改系统代理、VPN、DNS、hosts、证书、端口 443 或用户网络工具。",
    "5. 不执行 wsl --shutdown、wsl --terminate、wsl --unregister；不影响 2222 或其他无关服务。",
    "6. 若我批准修改，只备份 Bridge 自己的 config.json（0600），再通过本地 /api/config 局部 POST 一个字段；验证失败必须从备份回滚。不关闭 Claude Science 启动器 GUI。",
    "7. 不额外发送模型请求。需要重跑真实请求前，必须再次说明 max_tokens=1 和可能产生的极少量费用并取得确认。",
    "",
    "验证方式：",
    "- 先在 WSL 内验证代理 host:port 的 TCP 状态，以及直连上游 DNS/TCP/TLS/HTTP 是否可达；不带 Key 的 401/403 可作为传输可达证据。",
    "- 经批准完成最小修复、读回 /health 后，运行：",
    "  powershell -NoProfile -ExecutionPolicy Bypass -File .\\scripts\\csa-smoke.ps1 -Only \"bridge,egress\"",
    "- 预期 egress 从当前错误码转为 work.bridge_egress.ok；它始终属于 WORK 车道，即使仍失败也不得阻塞‘打开 Claude Science’。",
    "",
    "最后按以下格式汇报并停止：只读证据、根因、拟修改差异、风险与回退、验证结果；明确列出未改动的系统网络项和未触碰的无关服务。",
  ].join("\n");
}

export const browserPreviewBridgeEgressReport: BridgeEgressReport = {
  operation: "bridge_egress",
  ok: false,
  code: "work.bridge_egress.proxy_dead",
  conclusion: "Bridge 配置的 outbound proxy 在 WSL 内未监听；未发送真实模型请求。",
  billableRequestSent: false,
  outboundProxyConfigured: true,
  outboundProxyUrl: "http://127.0.0.1:10808",
  health: { state: "passed", code: "work.bridge_egress.health_ok", httpStatus: 200, durationMs: 18 },
  proxy: { state: "failed", code: "work.bridge_egress.proxy_dead", durationMs: 2 },
  models: { state: "skipped", code: "work.bridge_egress.models_skipped", durationMs: 0 },
  request: { state: "skipped", code: "work.bridge_egress.request_skipped", durationMs: 0 },
  direct: { state: "skipped", code: "work.bridge_egress.direct_skipped", durationMs: 0 },
  candidates: [
    {
      address: "http://127.0.0.1:12334",
      source: "windows_system_proxy",
      processName: "Hiddify",
      tcp: { state: "passed", code: "work.bridge_egress.candidate.tcp_ok", durationMs: 2 },
      upstream: { state: "passed", code: "work.bridge_egress.candidate.upstream_ok", httpStatus: 401, durationMs: 132 },
      recommended: true,
      reason: "Windows 系统代理与监听进程一致，且可到达当前上游。",
    },
    {
      address: "",
      source: "direct",
      tcp: { state: "passed", code: "work.bridge_egress.candidate.tcp_ok", durationMs: 0 },
      upstream: { state: "passed", code: "work.bridge_egress.candidate.upstream_ok", httpStatus: 401, durationMs: 171 },
      recommended: false,
      reason: "当前上游直连可达，但覆盖面窄，排在本地代理之后。",
    },
  ],
  suggestedAction: "将 outbound_proxy_url 置空，或替换为 WSL 内实际监听的代理地址。",
  warnings: [],
};
