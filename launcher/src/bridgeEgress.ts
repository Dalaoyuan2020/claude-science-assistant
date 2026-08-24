export type BridgeEgressLayerState = "passed" | "failed" | "skipped";

export interface BridgeEgressLayer {
  state: BridgeEgressLayerState;
  code: string;
  httpStatus?: number;
  durationMs: number;
  detail?: string;
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
  suggestedAction: string;
  warnings: string[];
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
  return /^work\.bridge_egress\.[a-z0-9_]+$/.test(code)
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
  const warnings = report.warnings.length > 0
    ? report.warnings.slice(0, 8).map((warning) => `- ${safeLine(warning)}`).join("\n")
    : "- 无额外警告";

  return [
    "请协助我只读复核并修复 CSA Bridge → 上游模型 API 的真实出口。当前目录应是 CSA V0.1.6 完整便携包或其源码工作树。",
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
    "建议动作：",
    proxyDead
      ? "- 已命中 work.bridge_egress.proxy_dead。请先证明该代理在 Bridge 所在 WSL 网络命名空间中确实无人监听，再提出把 outbound_proxy_url 置空，或替换成 WSL 内实际监听地址的最小配置差异。"
      : `- ${safeLine(report.suggestedAction)}`,
    "- 先展示拟修改字段、修改前后的脱敏值、备份位置、Bridge-only 重启方式和回退方式，然后停止等待我明确批准。",
    "",
    "硬性红线：",
    "1. 本轮先只读复核；未经我明确批准，不修改 outbound_proxy_url 或任何配置。",
    "2. 不读取、输出、复制或记录 API Key、token、私钥、Cookie、请求头、响应正文或代理凭据。",
    "3. 不调用可能回显短 Key 或未脱敏代理 URL 的 /api/config；只使用 /health 的脱敏字段和 WSL 内本地只读检查。",
    "4. 不修改系统代理、VPN、DNS、hosts、证书、端口 443 或用户网络工具。",
    "5. 不执行 wsl --shutdown、wsl --terminate、wsl --unregister；不影响 2222 或其他无关服务。",
    "6. 若我批准修改，只备份并原子修改 Bridge 自己的 config.json，备份权限保持 0600；只重启 Bridge，不关闭 Claude Science 启动器 GUI。",
    "7. 不额外发送模型请求。需要重跑真实请求前，必须再次说明 max_tokens=1 和可能产生的极少量费用并取得确认。",
    "",
    "验证方式：",
    "- 先在 WSL 内验证代理 host:port 的 TCP 状态，以及直连上游 DNS/TCP/TLS/HTTP 是否可达；不带 Key 的 401/403 可作为传输可达证据。",
    "- 经批准完成最小修复和 Bridge-only 重启后，运行：",
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
  suggestedAction: "将 outbound_proxy_url 置空，或替换为 WSL 内实际监听的代理地址。",
  warnings: [],
};
