import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { buildStorageMigrationPrompt, storageRecommendation } from "./storageMigration";
import {
  browserPreviewBridgeEgressReport,
  buildBridgeEgressRepairPrompt,
  displayBridgeEgressAddress,
  displayBridgeEgressCandidateAddress,
  type BridgeEgressApplyResult,
  type BridgeEgressCandidate,
  type BridgeEgressLayer,
  type BridgeEgressReport,
} from "./bridgeEgress";
import {
  browserPreviewRuntimeStatus,
  buildRuntimeRollbackPrompt,
  buildRuntimeUpgradePrompt,
  type RuntimeUpdateStatus,
} from "./runtimeUpdate";
import { HelpDialog } from "./HelpDialog";
import {
  classifyNonGatingFailure,
  createLaneState,
  createProbeCircuit,
  laneReducer,
  nonGatingFailurePresentation,
  nonGatingFailureIsMuted,
  primaryButtonView,
  runNonGatingProbe,
  type AllowStatus,
  type LaneState,
} from "./laneContract";
import { deleteConfirmationText, screenMessage } from "./uiPresentation";
import "./App.css";

const APP_VERSION = "v0.1.7";
const GRADE_STATUS_TIMEOUT_MS = 15_000;
const NETWORK_QUALITY_TIMEOUT_MS = 25_000;
const RUNTIME_UPDATE_TIMEOUT_MS = 45_000;
const BRIDGE_EGRESS_TIMEOUT_MS = 75_000;
const SCREEN_LINE_INTERVAL_MS = 3_000;
const SCREEN_LINE_LIMIT = 40;
const STARTUP_CAPTIONS = [
  "检查 WSL2",
  "启动 Bridge",
  "校验运行时身份",
  "启动 Claude Science",
  "等待控制通道",
] as const;

type SystemState = "loading" | "notInstalled" | "stopped" | "degraded" | "running" | "error";
type UiSkin = "console" | "classic" | "dark";
type ScreenTone = "normal" | "warning" | "fault" | "safe";

interface UiPreferences {
  skin?: string;
}

interface ScreenLine {
  id: number;
  time: string;
  key: string;
  value: string;
  tone: ScreenTone;
}

interface ScreenCandidate {
  key: string;
  value: string;
  tone?: ScreenTone;
}

interface NetworkQualityStatus {
  proxyState: "unknown" | "not_running" | "direct" | "reachable" | "unreachable" | "conflict" | "invalid";
  localReady: boolean;
  ready: boolean;
  proxyReachable?: boolean;
  proxyEndpoints: string[];
  proxyConflict: boolean;
  sandboxForwarderCount: number;
  sandboxForwarderExpectedCount: number;
  sandboxForwarderTopologyState: string;
  sandboxHttpForwarderCount: number;
  sandboxSocksForwarderCount: number;
  sandboxProbeRole: string;
  sandboxProbeTransport: string;
  daemonProcessState: string;
  daemonWaitChannel: string;
  daemonIoBlocked: boolean;
  daemonMountIoBlocked: boolean;
  sandboxUnixSocketState: string;
  sandboxSocksHandshakeState: string;
  sandboxEgressFailureStage: string;
  deepChecked: boolean;
  deepCheckedAtUnix?: number;
  sandboxEgressState: string;
  sandboxEgressTarget?: string;
  sandboxEgressHttpStatus?: number;
}

interface DeepNetworkQualityStatus {
  deepChecked: boolean;
  deepCheckedAtUnix?: number;
  sandboxUnixSocketState: string;
  sandboxSocksHandshakeState: string;
  sandboxEgressFailureStage: string;
  sandboxEgressState: string;
  sandboxEgressTarget?: string;
  sandboxEgressHttpStatus?: number;
}

interface WorkReport {
  operation: string;
  ok: boolean;
  code: string;
  deep?: DeepNetworkQualityStatus;
  warnings: string[];
}

type WorkLaneValue =
  | { probe: "network_quality"; value?: WorkReport; errorCode?: string }
  | { probe: "runtime_update"; value?: RuntimeUpdateStatus; errorCode?: string }
  | { probe: "bridge_egress"; value?: BridgeEgressReport; errorCode?: string };

interface NonGatingProbeNotice {
  source: "grade.status" | "work.network_quality" | "work.runtime_update" | "work.bridge_egress";
  code: string;
  message: string;
  muted: boolean;
}

interface GradeNetworkStatus {
  proxyState: NetworkQualityStatus["proxyState"];
  localReady: boolean;
  proxyReachable?: boolean;
  proxyEndpoints: string[];
  proxyConflict: boolean;
  sandboxForwarderCount: number;
  sandboxForwarderExpectedCount: number;
  sandboxForwarderTopologyState: string;
  sandboxHttpForwarderCount: number;
  sandboxSocksForwarderCount: number;
  sandboxProbeRole: string;
  sandboxProbeTransport: string;
  daemonProcessState: string;
  daemonWaitChannel: string;
  daemonIoBlocked: boolean;
  daemonMountIoBlocked: boolean;
}

interface GradeStatus {
  state: SystemState;
  wslInstalled: boolean;
  bridgeRunning: boolean;
  bridgePid?: number;
  bridgeHealthy: boolean;
  bridgeIdentity?: unknown;
  claudeListenerPresent: boolean;
  claudePort8765: boolean;
  claudePort8766: boolean;
  claudeUnverifiedPid?: number;
  runtimeReady: boolean;
  sourceBinaryPresent: boolean;
  bridgeVenvPresent: boolean;
  wslStoragePath?: string;
  wslStorageDrive?: string;
  wslStorageFreeGb?: number;
  wslVhdxSizeGb?: number;
  wslRootFreeGb?: number;
  settingsStorageDrive?: string;
  settingsStorageFreeGb?: number;
  storageWarning: boolean;
  storageBlocked: boolean;
  restartBlocked: boolean;
  hostAccessPreferencesPresent: boolean;
  hostAccessPreferencesParseOk: boolean;
  drvfsWriteGrantCount: number;
  broadDrvfsWriteGrantCount: number;
  drvfsWriteGrants: string[];
  network: GradeNetworkStatus;
  warnings: string[];
}

interface SystemStatus {
  state: SystemState;
  wslInstalled: boolean;
  distro?: string;
  linuxUser?: string;
  bridgeRunning: boolean;
  bridgePid?: number;
  claudeRunning: boolean;
  claudePid?: number;
  claudeListenerPresent: boolean;
  bridgeHealthy: boolean;
  windowsBridgePid?: number;
  runtimeReady: boolean;
  sourceBinaryPresent: boolean;
  bridgeVenvPresent: boolean;
  wslStoragePath?: string;
  wslStorageDrive?: string;
  wslStorageFreeGb?: number;
  wslVhdxSizeGb?: number;
  wslRootFreeGb?: number;
  settingsStorageDrive?: string;
  settingsStorageFreeGb?: number;
  storageWarning: boolean;
  storageBlocked: boolean;
  restartBlocked: boolean;
  network: NetworkQualityStatus;
  warnings: string[];
}

const mergeGradeStatus = (current: SystemStatus, next: GradeStatus): SystemStatus => ({
  ...current,
  state: next.state,
  bridgeRunning: next.bridgeRunning,
  bridgePid: next.bridgePid,
  bridgeHealthy: next.bridgeHealthy,
  claudeListenerPresent: next.claudeListenerPresent,
  runtimeReady: next.runtimeReady,
  sourceBinaryPresent: next.sourceBinaryPresent,
  bridgeVenvPresent: next.bridgeVenvPresent,
  wslStoragePath: next.wslStoragePath,
  wslStorageDrive: next.wslStorageDrive,
  wslStorageFreeGb: next.wslStorageFreeGb,
  wslVhdxSizeGb: next.wslVhdxSizeGb,
  wslRootFreeGb: next.wslRootFreeGb,
  settingsStorageDrive: next.settingsStorageDrive,
  settingsStorageFreeGb: next.settingsStorageFreeGb,
  storageWarning: next.storageWarning,
  storageBlocked: next.storageBlocked,
  restartBlocked: next.restartBlocked,
  network: {
    ...current.network,
    ...next.network,
    ready: next.network.localReady
      && !next.network.daemonIoBlocked
      && current.network.deepChecked
      && current.network.sandboxEgressState === "ok",
  },
  warnings: next.warnings,
});

interface Provider {
  id: string;
  name: string;
  meta: string;
  badge: "官方" | "聚合" | "中转" | "自建" | "自定义";
  trust: string;
  protocol: string;
  baseUrl?: string;
  defaultModel?: string;
}

interface ProviderGroup {
  title: string;
  tier: string;
  providers: Provider[];
}

interface ModelAlias {
  id: string;
  displayName: string;
  model: string;
}

interface ApiKeyEntry {
  id: string;
  providerId: string;
  label: string;
  baseUrl: string;
  model: string;
  customConfirmed: boolean;
  modelAliases: ModelAlias[];
  hasSecret: boolean;
  active: boolean;
}

type SubscriptionRole = "default" | "vision" | "fast";

type DraftRoleModels = Record<SubscriptionRole, string>;

type AccessMode = "api" | "aggregate";

interface RoleBinding {
  role: SubscriptionRole;
  providerId: string;
  apiKeyId: string;
  model: string;
}

interface AggregateScheme {
  id: "scheme-1" | "scheme-2";
  name: string;
  routes: RoleBinding[];
}

type AggregateSchemeId = AggregateScheme["id"];

interface LauncherSettings {
  selectedProviderId: string;
  customBaseUrl: string;
  customConfirmed: boolean;
  activeApiKeyId?: string;
  apiKeys: ApiKeyEntry[];
  activeRole?: SubscriptionRole;
  roleBindings: RoleBinding[];
  activeAggregateSchemeId?: string;
  aggregateSchemes: AggregateScheme[];
}

interface ApiKeyTestResult {
  ok: boolean;
  providerId: string;
  baseUrl: string;
  upstreamMode: string;
  selectedModel: string;
  reply: string;
  models: string[];
  message: string;
}

interface ApiKeyAutoMapResult {
  ok: boolean;
  providerId: string;
  baseUrl: string;
  upstreamMode: string;
  primaryModel: string;
  fastModel: string;
  aliases: ModelAlias[];
  models: string[];
  message: string;
}

const fallbackProviderGroups: ProviderGroup[] = [
  {
    title: "官方直连",
    tier: "official",
    providers: [
      { id: "glm", name: "GLM-5.2", meta: "智谱官方 API", badge: "官方", trust: "official", protocol: "openai-compatible", baseUrl: "https://open.bigmodel.cn/api/paas/v4" },
      { id: "longcat", name: "LongCat", meta: "OpenAI / Anthropic 兼容", badge: "官方", trust: "official", protocol: "openai-compatible", baseUrl: "https://api.longcat.chat/openai" },
      { id: "deepseek", name: "DeepSeek", meta: "官方 API", badge: "官方", trust: "official", protocol: "anthropic-compatible", baseUrl: "https://api.deepseek.com/anthropic" },
      { id: "minimax", name: "MiniMax", meta: "中国区官方 API / Anthropic 兼容", badge: "官方", trust: "official", protocol: "anthropic-compatible", baseUrl: "https://api.minimaxi.com/anthropic" },
      { id: "claude", name: "Claude", meta: "官方登录 / API", badge: "官方", trust: "official", protocol: "official-login-or-api" },
      { id: "openai", name: "OpenAI / GPT", meta: "官方登录 / API", badge: "官方", trust: "official", protocol: "official-login-or-api", baseUrl: "https://api.openai.com/v1" },
    ],
  },
  {
    title: "聚合与编程订阅",
    tier: "aggregator",
    providers: [
      { id: "opencode-go", name: "OpenCode Go", meta: "订阅 API Key", badge: "聚合", trust: "aggregator", protocol: "openai-compatible", baseUrl: "https://opencode.ai/zen/go/v1" },
      { id: "openrouter", name: "OpenRouter", meta: "多模型路由", badge: "聚合", trust: "aggregator", protocol: "openai-compatible", baseUrl: "https://openrouter.ai/api/v1" },
    ],
  },
  {
    title: "中转服务",
    tier: "custom",
    providers: [
      { id: "builtin-relay", name: "项目方自建中转", meta: "10521052.xyz/v1 · 非模型厂商官方 API", badge: "自建", trust: "untrusted-builtin", protocol: "openai-compatible", baseUrl: "https://10521052.xyz/v1" },
      { id: "custom", name: "自定义中转", meta: "用户填写 Base URL", badge: "自定义", trust: "untrusted-custom", protocol: "openai-compatible" },
    ],
  },
];

const fallbackSettings: LauncherSettings = {
  selectedProviderId: "deepseek",
  customBaseUrl: "",
  customConfirmed: false,
  apiKeys: [],
  roleBindings: [],
  aggregateSchemes: [],
};

const roleDefinitions: { role: SubscriptionRole; label: string; detail: string }[] = [
  { role: "default", label: "决策", detail: "Opus · 深度思考" },
  { role: "vision", label: "视觉", detail: "Sonnet · 多模态" },
  { role: "fast", label: "日常", detail: "Haiku / Fast · 快速响应" },
];

const initialAllowStatus: AllowStatus = {
  wslInstalled: false,
  runtimePresent: false,
  claudeRunning: false,
  listenerPresent: false,
  daemonState: "unknown",
  listenerProbeOk: false,
  controlSocketPresent: false,
  windowsBridgeProbe: "unknown",
  canOpen: false,
  canStart: false,
};

const browserPreviewAllowStatus: AllowStatus = {
  ...initialAllowStatus,
  wslInstalled: true,
  distro: "Ubuntu-24.04",
  linuxUser: "preview",
  runtimePresent: true,
  listenerProbeOk: true,
  windowsBridgeProbe: "checked",
};

const initialStatus: SystemStatus = {
  state: "loading",
  wslInstalled: false,
  bridgeRunning: false,
  claudeRunning: false,
  claudeListenerPresent: false,
  bridgeHealthy: false,
  runtimeReady: false,
  sourceBinaryPresent: false,
  bridgeVenvPresent: false,
  storageWarning: false,
  storageBlocked: false,
  restartBlocked: false,
  network: {
    proxyState: "unknown",
    localReady: false,
    ready: false,
    proxyEndpoints: [],
    proxyConflict: false,
    sandboxForwarderCount: 0,
    sandboxForwarderExpectedCount: 3,
    sandboxForwarderTopologyState: "incomplete",
    sandboxHttpForwarderCount: 0,
    sandboxSocksForwarderCount: 0,
    sandboxProbeRole: "analysis",
    sandboxProbeTransport: "socks5h",
    daemonProcessState: "unknown",
    daemonWaitChannel: "unknown",
    daemonIoBlocked: false,
    daemonMountIoBlocked: false,
    sandboxUnixSocketState: "not_checked",
    sandboxSocksHandshakeState: "not_checked",
    sandboxEgressFailureStage: "not_checked",
    deepChecked: false,
    sandboxEgressState: "not_checked",
  },
  warnings: [],
};

const browserPreviewStatus: SystemStatus = {
  state: "stopped",
  wslInstalled: true,
  distro: "Ubuntu-24.04",
  linuxUser: "preview",
  bridgeRunning: false,
  claudeRunning: false,
  claudeListenerPresent: false,
  bridgeHealthy: false,
  runtimeReady: true,
  sourceBinaryPresent: true,
  bridgeVenvPresent: true,
  wslStoragePath: "E:\\WSL\\Ubuntu-24.04",
  wslStorageDrive: "E:",
  wslStorageFreeGb: 420,
  wslVhdxSizeGb: 48,
  wslRootFreeGb: 390,
  settingsStorageDrive: "C:",
  settingsStorageFreeGb: 80,
  storageWarning: false,
  storageBlocked: false,
  restartBlocked: false,
  network: {
    proxyState: "not_running",
    localReady: false,
    ready: false,
    proxyEndpoints: [],
    proxyConflict: false,
    sandboxForwarderCount: 0,
    sandboxForwarderExpectedCount: 3,
    sandboxForwarderTopologyState: "incomplete",
    sandboxHttpForwarderCount: 0,
    sandboxSocksForwarderCount: 0,
    sandboxProbeRole: "analysis",
    sandboxProbeTransport: "socks5h",
    daemonProcessState: "unknown",
    daemonWaitChannel: "unknown",
    daemonIoBlocked: false,
    daemonMountIoBlocked: false,
    sandboxUnixSocketState: "not_checked",
    sandboxSocksHandshakeState: "not_checked",
    sandboxEgressFailureStage: "not_checked",
    deepChecked: false,
    sandboxEgressState: "not_checked",
  },
  warnings: [],
};

const stateText: Record<SystemState, { title: string; detail: string }> = {
  loading: { title: "正在启动核心服务", detail: "优先确保 Claude Science 就绪，并检查 WSL 与 Bridge 依赖…" },
  notInstalled: { title: "环境尚未就绪", detail: "需要用体检 Skill 安装或修复 WSL2 / Claude Science 运行环境" },
  stopped: { title: "Claude Science 已停止", detail: "环境完整，可以安全启动" },
  degraded: { title: "服务需要修复", detail: "部分组件正在运行，请查看诊断信息" },
  running: { title: "Claude Science 已准备好", detail: "Bridge、本地端口与沙盒真实出口均已验证" },
  error: { title: "无法读取系统状态", detail: "请查看错误详情后重试" },
};

const badgeClass: Record<Provider["badge"], string> = {
  官方: "official",
  聚合: "aggregator",
  中转: "relay",
  自建: "relay",
  自定义: "custom",
};

const providerInitial = (provider?: Provider) => {
  if (!provider) return "?";
  if (provider.id === "opencode-go") return "GO";
  if (provider.id === "openrouter") return "OR";
  if (provider.id === "minimax") return "MM";
  if (provider.id === "builtin-relay") return "↔";
  if (provider.id === "custom") return "+";
  return provider.name.slice(0, 1).toUpperCase();
};

const providerList = (groups: ProviderGroup[]) => groups.flatMap((group) => group.providers);

const previewCustomRelayLabel = (entries: ApiKeyEntry[], requestedName: string) => {
  const requested = requestedName.trim();
  if (requested) return requested;
  const date = new Date().toLocaleDateString("sv-SE");
  const prefix = `自定义中转 ${date} #`;
  const next = entries.reduce((highest, entry) => {
    if (entry.providerId !== "custom" || !entry.label.startsWith(prefix)) return highest;
    const sequence = Number.parseInt(entry.label.slice(prefix.length), 10);
    return Number.isFinite(sequence) ? Math.max(highest, sequence) : highest;
  }, 0) + 1;
  return `${prefix}${String(next).padStart(2, "0")}`;
};

const previewLabelForProvider = (
  entries: ApiKeyEntry[],
  provider: Provider,
  requestedName: string,
) => {
  const requested = requestedName.trim();
  if (requested) return requested;
  return provider.id === "custom"
    ? previewCustomRelayLabel(entries, "")
    : provider.name;
};

const screenTime = () => new Intl.DateTimeFormat("zh-CN", {
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hour12: false,
}).format(new Date());

const initialHealthCollapsed = () => {
  try {
    return window.localStorage.getItem("csa-health-collapsed") === "1";
  } catch {
    return false;
  }
};

const rememberHealthCollapsed = (value: boolean) => {
  try {
    window.localStorage.setItem("csa-health-collapsed", value ? "1" : "0");
  } catch {
    // The launcher remains usable when WebView storage is disabled or unavailable.
  }
};

const initialApiSectionCollapsed = () => {
  try {
    return window.localStorage.getItem("csa-api-section-collapsed") === "1";
  } catch {
    return false;
  }
};

const rememberApiSectionCollapsed = (value: boolean) => {
  try {
    window.localStorage.setItem("csa-api-section-collapsed", value ? "1" : "0");
  } catch {
    // The launcher remains usable when WebView storage is disabled or unavailable.
  }
};

const modelsForApiKey = (entry?: ApiKeyEntry) => {
  if (!entry) return [];
  return [entry.model, ...(entry.modelAliases || []).map((alias) => alias.model)]
    .map((model) => model.trim())
    .filter((model, index, models) => Boolean(model) && models.indexOf(model) === index);
};

const emptyDraftRoleModels = (): DraftRoleModels => ({ default: "", vision: "", fast: "" });

const uniqueModels = (...groups: (string[] | undefined)[]) => groups
  .flatMap((group) => group || [])
  .map((model) => model.trim())
  .filter((model, index, models) => Boolean(model) && models.indexOf(model) === index);

const inferDraftRoleModels = (models: string[], primaryModel = "", fastModel = ""): DraftRoleModels => {
  const available = uniqueModels(models, [primaryModel, fastModel]);
  const fallback = primaryModel || available[0] || "";
  const decision = available.find((model) => /opus|reason|thinking|deep|pro|max|(^|[-_/])r1($|[-_/])|(^|[-_/])o[1-9]($|[-_/])/i.test(model)) || fallback;
  const vision = available.find((model) => /vision|(^|[-_/])vl($|[-_/])|image|multimodal|4o/i.test(model)) || fallback;
  const daily = available.find((model) => /fast|haiku|flash|mini|turbo|lite|speed/i.test(model)) || fastModel || fallback;
  return { default: decision, vision, fast: daily };
};

const aliasesForDraftRoleModels = (models: DraftRoleModels): ModelAlias[] => [
  { id: "byok-model-0001", displayName: `CSA 决策模型 -> ${models.default}`, model: models.default },
  { id: "claude-opus-4-8", displayName: `Claude Opus / 决策 -> ${models.default}`, model: models.default },
  { id: "claude-sonnet-5", displayName: `Claude Sonnet / 视觉 -> ${models.vision}`, model: models.vision },
  { id: "claude-sonnet-4-5", displayName: `Claude Sonnet 4.5 / 视觉 -> ${models.vision}`, model: models.vision },
  { id: "claude-haiku-4-5-20251001", displayName: `Claude Haiku / 快速 -> ${models.fast}`, model: models.fast },
].filter((alias) => Boolean(alias.model));

const suggestedRoleModel = (entry: ApiKeyEntry | undefined, role: SubscriptionRole) => {
  const models = modelsForApiKey(entry);
  if (models.length === 0) return "";
  if (role === "fast") {
    const alias = (entry?.modelAliases || []).find((item) => /fast|haiku|flash|highspeed/i.test(`${item.id} ${item.displayName} ${item.model}`));
    if (alias?.model) return alias.model;
  }
  if (role === "vision") {
    const visual = models.find((model) => /vision|vl|image|multimodal|4o/i.test(model));
    if (visual) return visual;
  }
  if (role === "default") {
    const decision = models.find((model) => /opus|reason|thinking|deep|pro|max|(^|[-_/])r1($|[-_/])|(^|[-_/])o[1-9]($|[-_/])/i.test(model));
    if (decision) return decision;
  }
  return entry?.model || models[0];
};

const roleDraftsFromSettings = (settings: LauncherSettings, savedRoutes?: RoleBinding[]): RoleBinding[] => {
  const eligible = (settings.apiKeys || []).filter((entry) => entry.hasSecret && modelsForApiKey(entry).length > 0);
  const fallback = eligible.find((entry) => entry.id === settings.activeApiKeyId) || eligible[0];
  return roleDefinitions.map(({ role }) => {
    const saved = (savedRoutes || settings.roleBindings || []).find((binding) => binding.role === role);
    if (saved) return saved;
    return {
      role,
      providerId: fallback?.providerId || "",
      apiKeyId: fallback?.id || "",
      model: suggestedRoleModel(fallback, role),
    };
  });
};

const normalizedSchemes = (settings: LauncherSettings): AggregateScheme[] => {
  const stored = settings.aggregateSchemes || [];
  return (["scheme-1", "scheme-2"] as const).map((id, index) => {
    const scheme = stored.find((item) => item.id === id);
    return {
      id,
      name: index === 0 ? "方案一" : "方案二",
      routes: scheme?.routes || (index === 0 ? settings.roleBindings || [] : []),
    };
  });
};

function App() {
  const [skin, setSkin] = useState<UiSkin>("console");
  const [skinPreferenceResolved, setSkinPreferenceResolved] = useState(false);
  const [showSkinChooser, setShowSkinChooser] = useState(false);
  const [showHelp, setShowHelp] = useState(false);
  const [skinChoiceRequired, setSkinChoiceRequired] = useState(false);
  const [skinSaving, setSkinSaving] = useState(false);
  const [skinError, setSkinError] = useState("");
  const [status, setStatus] = useState<SystemStatus>(initialStatus);
  const [allowStatus, setAllowStatus] = useState<AllowStatus>(initialAllowStatus);
  const [allowLoaded, setAllowLoaded] = useState(false);
  const [providerGroups, setProviderGroups] = useState<ProviderGroup[]>(fallbackProviderGroups);
  const [activeProvider, setActiveProvider] = useState(fallbackSettings.selectedProviderId);
  const [customBaseUrl, setCustomBaseUrl] = useState(fallbackSettings.customBaseUrl);
  const [customConfirmed, setCustomConfirmed] = useState(fallbackSettings.customConfirmed);
  const [activeApiKeyId, setActiveApiKeyId] = useState<string | undefined>();
  const [pendingApiKeyId, setPendingApiKeyId] = useState<string | undefined>();
  const [apiKeys, setApiKeys] = useState<ApiKeyEntry[]>(fallbackSettings.apiKeys);
  const [activeRole, setActiveRole] = useState<SubscriptionRole | undefined>();
  const [activeAggregateSchemeId, setActiveAggregateSchemeId] = useState<AggregateSchemeId | undefined>();
  const [accessMode, setAccessMode] = useState<AccessMode>("api");
  const [aggregateSchemes, setAggregateSchemes] = useState<AggregateScheme[]>(normalizedSchemes(fallbackSettings));
  const [selectedSchemeId, setSelectedSchemeId] = useState<AggregateSchemeId>("scheme-1");
  const [pendingSchemeId, setPendingSchemeId] = useState<AggregateSchemeId>("scheme-1");
  const [roleBindings, setRoleBindings] = useState<RoleBinding[]>(roleDraftsFromSettings(fallbackSettings));
  const [roleMappingsDirty, setRoleMappingsDirty] = useState(false);
  const [showKeyPicker, setShowKeyPicker] = useState(false);
  const [draftProviderId, setDraftProviderId] = useState(fallbackSettings.selectedProviderId);
  const [draftApiKey, setDraftApiKey] = useState("");
  const [draftDisplayName, setDraftDisplayName] = useState("");
  const [draftBaseUrl, setDraftBaseUrl] = useState("");
  const [draftModel, setDraftModel] = useState("");
  const [draftModelAliases, setDraftModelAliases] = useState<ModelAlias[]>([]);
  const [draftRoleModels, setDraftRoleModels] = useState<DraftRoleModels>(emptyDraftRoleModels);
  const [draftAvailableModels, setDraftAvailableModels] = useState<string[]>([]);
  const [draftConfirmed, setDraftConfirmed] = useState(false);
  const [deleteConfirmApiKeyId, setDeleteConfirmApiKeyId] = useState<string>();
  const [renameApiKeyId, setRenameApiKeyId] = useState<string>();
  const [renameDisplayName, setRenameDisplayName] = useState("");
  const [testPrompt, setTestPrompt] = useState("Reply only: OK");
  const [testResult, setTestResult] = useState<ApiKeyTestResult | undefined>();
  const [autoMapResult, setAutoMapResult] = useState<ApiKeyAutoMapResult | undefined>();
  const [testingKey, setTestingKey] = useState(false);
  const [autoMappingKey, setAutoMappingKey] = useState(false);
  const [busy, setBusy] = useState(false);
  const [allowActionBusy, setAllowActionBusy] = useState(false);
  const [repairBusy, setRepairBusy] = useState(false);
  const [networkChecking, setNetworkChecking] = useState(false);
  const [workWarnings, setWorkWarnings] = useState<string[]>([]);
  const [probeNotice, setProbeNotice] = useState<NonGatingProbeNotice>();
  const [error, setError] = useState("");
  const [healthCollapsed, setHealthCollapsed] = useState(initialHealthCollapsed);
  const [apiSectionCollapsed, setApiSectionCollapsed] = useState(initialApiSectionCollapsed);
  const [showMigrationAssistant, setShowMigrationAssistant] = useState(false);
  const [migrationCopyState, setMigrationCopyState] = useState("");
  const [runtimeUpdate, setRuntimeUpdate] = useState<RuntimeUpdateStatus>();
  const [runtimeChecking, setRuntimeChecking] = useState(false);
  const [runtimeError, setRuntimeError] = useState("");
  const [runtimePromptMode, setRuntimePromptMode] = useState<"upgrade" | "rollback">();
  const [runtimeCopyState, setRuntimeCopyState] = useState("");
  const [showBridgeEgressAssistant, setShowBridgeEgressAssistant] = useState(false);
  const [bridgeEgressReport, setBridgeEgressReport] = useState<BridgeEgressReport>();
  const [bridgeEgressChecking, setBridgeEgressChecking] = useState(false);
  const [bridgeEgressError, setBridgeEgressError] = useState("");
  const [bridgeEgressCopyState, setBridgeEgressCopyState] = useState("");
  const [bridgeEgressApplyBusy, setBridgeEgressApplyBusy] = useState(false);
  const [bridgeEgressApplyError, setBridgeEgressApplyError] = useState("");
  const [bridgeEgressApplyResult, setBridgeEgressApplyResult] = useState<BridgeEgressApplyResult>();
  const [screenLines, setScreenLines] = useState<ScreenLine[]>([]);
  const [screenPaused, setScreenPaused] = useState(false);
  const [screenHoverPaused, setScreenHoverPaused] = useState(false);
  const [startupCaptionIndex, setStartupCaptionIndex] = useState(0);
  const allowRefreshEpoch = useRef(0);
  const gradeRefreshInFlight = useRef(false);
  const busyRef = useRef(false);
  const allowActionBusyRef = useRef(false);
  const repairBusyRef = useRef(false);
  const networkCheckingRef = useRef(false);
  const statusCommitEpoch = useRef(0);
  const runtimeInitializationAttempted = useRef(false);
  const runtimeCheckingRef = useRef(false);
  const bridgeEgressCheckingRef = useRef(false);
  const bridgeEgressApplyBusyRef = useRef(false);
  const bridgeEgressDialogRef = useRef<HTMLElement>(null);
  const deleteCancelRef = useRef<HTMLButtonElement>(null);
  const deleteButtonRefs = useRef(new Map<string, HTMLButtonElement>());
  const renameInputRef = useRef<HTMLInputElement>(null);
  const screenLogRef = useRef<HTMLDivElement>(null);
  const screenLineIdRef = useRef(0);
  const screenCandidateIndexRef = useRef(0);
  const laneStateRef = useRef<LaneState<AllowStatus, SystemStatus, WorkLaneValue | undefined>>(
    createLaneState(initialAllowStatus, initialStatus, undefined),
  );
  const gradeStatusCircuitRef = useRef(createProbeCircuit());
  const networkQualityCircuitRef = useRef(createProbeCircuit());
  const runtimeUpdateCircuitRef = useRef(createProbeCircuit());
  const bridgeEgressCircuitRef = useRef(createProbeCircuit());

  const isTauri = "__TAURI_INTERNALS__" in window;
  const providers = useMemo(() => providerList(providerGroups), [providerGroups]);
  const activeKeyEntry = apiKeys.find((entry) => entry.id === activeApiKeyId);
  const activeKeyProvider = providers.find((provider) => provider.id === (activeKeyEntry?.providerId || activeProvider)) || providers[0];
  const pendingScheme = aggregateSchemes.find((scheme) => scheme.id === pendingSchemeId);
  const pendingSchemeComplete = Boolean(
    pendingScheme
    && pendingScheme.routes.length === roleDefinitions.length
    && pendingScheme.routes.every((route) => route.apiKeyId && route.providerId && route.model),
  );
  const roleEligibleKeys = useMemo(
    () => apiKeys.filter((entry) => entry.hasSecret && modelsForApiKey(entry).length > 0),
    [apiKeys],
  );
  const draftProvider = providers.find((provider) => provider.id === draftProviderId) || activeKeyProvider;
  const draftNeedsBaseUrl = draftProvider?.id === "custom";
  const draftIsThirdParty = draftProvider?.trust.startsWith("untrusted") || false;
  const draftModelOptions = uniqueModels(
    draftAvailableModels,
    [draftModel],
    Object.values(draftRoleModels),
  );
  const deepNetworkReady = status.network.deepChecked && status.network.sandboxEgressState === "ok";
  const primaryButton = useMemo(
    () => primaryButtonView(allowStatus, allowLoaded, allowActionBusy),
    [allowActionBusy, allowLoaded, allowStatus],
  );
  const canOpenClaude = primaryButton.action === "open";
  const mutationBusy = busy || networkChecking;
  const appearanceBlocked = busy || allowActionBusy || repairBusy;
  const baseSummary = status.state === "running" && !deepNetworkReady
    ? {
      title: "Claude Science 已准备好",
      detail: "Bridge、本地端口与 3/3 沙盒出口拓扑已验证；外部 API 深度质检不影响打开",
    }
    : stateText[status.state];
  const summary = canOpenClaude && ["degraded", "error"].includes(status.state)
    ? { ...baseSummary, detail: nonGatingFailurePresentation(baseSummary.detail, allowStatus) }
    : baseSummary;
  const storageStatus = useMemo(
    () => ({ ...status, distro: allowStatus.distro }),
    [allowStatus.distro, status],
  );
  const migrationRecommendation = useMemo(() => storageRecommendation(storageStatus), [storageStatus]);
  const migrationPrompt = useMemo(() => buildStorageMigrationPrompt(storageStatus), [storageStatus]);
  const runtimePrompt = useMemo(() => {
    if (!runtimeUpdate || !runtimePromptMode) return "";
    return runtimePromptMode === "upgrade"
      ? buildRuntimeUpgradePrompt(runtimeUpdate)
      : buildRuntimeRollbackPrompt(runtimeUpdate);
  }, [runtimePromptMode, runtimeUpdate]);
  const bridgeEgressPrompt = useMemo(
    () => bridgeEgressReport ? buildBridgeEgressRepairPrompt(bridgeEgressReport) : "",
    [bridgeEgressReport],
  );
  const recommendedBridgeEgressCandidate = useMemo(
    () => bridgeEgressReport?.candidates?.find((candidate) => candidate.recommended),
    [bridgeEgressReport],
  );
  const closeHelp = useCallback(() => setShowHelp(false), []);

  useEffect(() => {
    document.documentElement.dataset.skin = skin;
  }, [skin]);

  const commitAllowStatus = useCallback((next: AllowStatus) => {
    const reduced = laneReducer(laneStateRef.current, { lane: "allow", value: next });
    laneStateRef.current = reduced;
    setAllowStatus(reduced.allow);
  }, []);

  const refreshAllow = useCallback(async () => {
    const requestEpoch = ++allowRefreshEpoch.current;
    const next = isTauri
      ? await invoke<AllowStatus>("get_allow_status")
      : browserPreviewAllowStatus;
    if (requestEpoch !== allowRefreshEpoch.current) return next;
    commitAllowStatus(next);
    setAllowLoaded(true);
    return next;
  }, [commitAllowStatus, isTauri]);

  const refreshGrade = useCallback(async () => {
    if (gradeRefreshInFlight.current) return;
    gradeRefreshInFlight.current = true;
    const requestEpoch = statusCommitEpoch.current;
    try {
      if (!isTauri) {
        const reduced = laneReducer(laneStateRef.current, { lane: "grade", value: browserPreviewStatus });
        laneStateRef.current = reduced;
        setStatus(reduced.grade);
        return;
      }
      const result = await runNonGatingProbe<GradeStatus>({
        lane: "grade",
        key: "status",
        timeoutMs: GRADE_STATUS_TIMEOUT_MS,
        circuit: gradeStatusCircuitRef.current,
        task: () => invoke<GradeStatus>("get_grade_status"),
        classifyValue: (value) => classifyNonGatingFailure(
          "grade",
          "status",
          value.warnings.join("\n"),
        ),
      });
      if (requestEpoch !== statusCommitEpoch.current) return;
      if (!result.ok) {
        setProbeNotice({
          source: "grade.status",
          code: result.code,
          message: result.message,
          muted: nonGatingFailureIsMuted(result.code, result.timedOut, result.skipped),
        });
        return;
      }
      setProbeNotice((current) => current?.source === "grade.status" ? undefined : current);
      setStatus((current) => {
        const merged = mergeGradeStatus(current, result.value);
        const reduced = laneReducer(laneStateRef.current, { lane: "grade", value: merged });
        laneStateRef.current = reduced;
        return reduced.grade;
      });
      if (result.value.state === "error") {
        setProbeNotice({
          source: "grade.status",
          code: "grade.status.failed",
          message: result.value.warnings.join("；") || "GRADE probes reported a confirmed failure",
          muted: false,
        });
      }
    } finally {
      gradeRefreshInFlight.current = false;
    }
  }, [isTauri]);

  const initializeRuntimeInBackground = useCallback(async () => {
    if (!isTauri || runtimeInitializationAttempted.current) return;
    runtimeInitializationAttempted.current = true;
    const requestEpoch = statusCommitEpoch.current;
    try {
      await invoke<unknown>("initialize_runtime");
    } catch (reason) {
      if (requestEpoch === statusCommitEpoch.current) setError(String(reason));
    }
    try {
      await refreshAllow();
    } catch (reason) {
      if (requestEpoch === statusCommitEpoch.current) setError(String(reason));
    }
  }, [isTauri, refreshAllow]);

  const refresh = useCallback(async () => {
    try {
      await refreshAllow();
    } catch (reason) {
      setError(String(reason));
    }
    void refreshGrade();
  }, [refreshAllow, refreshGrade]);

  useEffect(() => {
    let cancelled = false;
    let gradeTimer: number | undefined;

    async function paintAllowThenStartBackground() {
      try {
        await refreshAllow();
      } catch (reason) {
        setError(String(reason));
      }
      await new Promise<void>((resolve) => window.requestAnimationFrame(() => resolve()));
      if (cancelled) return;
      gradeTimer = window.setInterval(refreshGrade, 30_000);
      void initializeRuntimeInBackground();
      void refreshGrade();
    }

    async function loadProviderState() {
      if (!isTauri) return;
      try {
        const [catalog, settings] = await Promise.all([
          invoke<ProviderGroup[]>("get_provider_catalog"),
          invoke<LauncherSettings>("get_launcher_settings"),
        ]);
        setProviderGroups(catalog);
        applyLauncherState(settings);
        primeDraft(catalog, settings.selectedProviderId, settings.customBaseUrl, settings.customConfirmed);
      } catch (reason) {
        setError(String(reason));
      }
    }

    async function loadSkinPreference() {
      if (!isTauri) {
        setSkin("console");
        setSkinChoiceRequired(true);
        setShowSkinChooser(true);
        setSkinPreferenceResolved(true);
        return;
      }
      try {
        const preferences = await invoke<UiPreferences>("get_ui_preferences");
        if (preferences.skin === "classic" || preferences.skin === "console" || preferences.skin === "dark") {
          setSkin(preferences.skin);
          setSkinChoiceRequired(false);
          setShowSkinChooser(false);
          return;
        }
        if (preferences.skin === undefined || preferences.skin === null) {
          setSkin("console");
          setSkinChoiceRequired(true);
          setShowSkinChooser(true);
          return;
        }
        setSkin("console");
        setSkinChoiceRequired(false);
        setShowSkinChooser(false);
      } catch {
        // Appearance is presentation-only. A read failure must never blank or
        // gate the launcher, so continue with the console skin.
        setSkin("console");
        setSkinChoiceRequired(false);
        setShowSkinChooser(false);
      } finally {
        setSkinPreferenceResolved(true);
      }
    }
    void paintAllowThenStartBackground();
    void loadProviderState();
    void loadSkinPreference();
    return () => {
      cancelled = true;
      if (gradeTimer !== undefined) window.clearInterval(gradeTimer);
    };
  }, [initializeRuntimeInBackground, isTauri, refreshAllow, refreshGrade]);

  useEffect(() => {
    if (!showBridgeEgressAssistant) return;
    const previouslyFocused = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : undefined;
    const focusFrame = window.requestAnimationFrame(() => bridgeEgressDialogRef.current?.focus());
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !bridgeEgressCheckingRef.current) {
        event.preventDefault();
        setShowBridgeEgressAssistant(false);
      }
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      window.cancelAnimationFrame(focusFrame);
      document.removeEventListener("keydown", closeOnEscape);
      previouslyFocused?.focus();
    };
  }, [showBridgeEgressAssistant]);

  useEffect(() => {
    if (!deleteConfirmApiKeyId) return;
    const focusFrame = window.requestAnimationFrame(() => deleteCancelRef.current?.focus());
    return () => window.cancelAnimationFrame(focusFrame);
  }, [deleteConfirmApiKeyId]);

  useEffect(() => {
    if (!renameApiKeyId) return;
    const focusFrame = window.requestAnimationFrame(() => {
      renameInputRef.current?.focus();
      renameInputRef.current?.select();
    });
    return () => window.cancelAnimationFrame(focusFrame);
  }, [renameApiKeyId]);

  async function chooseSkin(nextSkin: UiSkin) {
    if (skinSaving || busyRef.current || allowActionBusyRef.current || repairBusyRef.current) return;
    setSkinSaving(true);
    setSkinError("");
    try {
      if (isTauri) {
        const saved = await invoke<UiPreferences>("save_ui_skin", { uiSkin: nextSkin });
        setSkin(saved.skin === "classic" || saved.skin === "dark" ? saved.skin : "console");
      } else {
        setSkin(nextSkin);
      }
      setSkinChoiceRequired(false);
      setShowSkinChooser(false);
    } catch (reason) {
      setSkinError(String(reason));
    } finally {
      setSkinSaving(false);
    }
  }

  function updateBusy(value: boolean) {
    if (value) {
      // Invalidate any slower periodic refresh that started before this user
      // action, so it cannot overwrite the action's newer status or error.
      statusCommitEpoch.current += 1;
    }
    busyRef.current = value;
    setBusy(value);
  }

  function updateAllowActionBusy(value: boolean) {
    if (value) {
      // ALLOW user actions invalidate older Grade commits without sharing their
      // busy flag with settings, Grade, or Work operations.
      statusCommitEpoch.current += 1;
    }
    allowActionBusyRef.current = value;
    setAllowActionBusy(value);
  }

  function tryBeginMutation() {
    if (busyRef.current || networkCheckingRef.current) return false;
    updateBusy(true);
    return true;
  }

  function applyLauncherState(settings: LauncherSettings) {
    setActiveProvider(settings.selectedProviderId);
    setCustomBaseUrl(settings.customBaseUrl);
    setCustomConfirmed(settings.customConfirmed);
    setActiveApiKeyId(settings.activeApiKeyId);
    setPendingApiKeyId(settings.activeApiKeyId);
    setApiKeys(settings.apiKeys || []);
    setActiveRole(settings.activeRole);
    const activeSchemeId = settings.activeAggregateSchemeId === "scheme-1" || settings.activeAggregateSchemeId === "scheme-2"
      ? settings.activeAggregateSchemeId
      : undefined;
    setActiveAggregateSchemeId(activeSchemeId);
    setAccessMode(activeSchemeId ? "aggregate" : "api");
    const schemes = normalizedSchemes(settings);
    const preferredId = activeSchemeId || selectedSchemeId;
    const selected = schemes.find((scheme) => scheme.id === preferredId) || schemes[0];
    setAggregateSchemes(schemes);
    setSelectedSchemeId(selected.id);
    setPendingSchemeId(activeSchemeId || selected.id);
    setRoleBindings(roleDraftsFromSettings(settings, selected.routes));
    setRoleMappingsDirty(
      selected.routes.length < roleDefinitions.length
      && (settings.apiKeys || []).some((entry) => entry.hasSecret && modelsForApiKey(entry).length > 0),
    );
  }

  function primeDraft(groups = providerGroups, providerId = activeProvider, baseUrl = customBaseUrl, confirmed = customConfirmed) {
    const provider = providerList(groups).find((item) => item.id === providerId) || providerList(groups)[0];
    setDraftProviderId(provider?.id || providerId);
    setDraftApiKey("");
    setDraftDisplayName("");
    setDraftBaseUrl(provider?.id === "custom" ? baseUrl : provider?.baseUrl || "");
    setDraftModel("");
    setDraftModelAliases([]);
    setDraftRoleModels(emptyDraftRoleModels());
    setDraftAvailableModels([]);
    setDraftConfirmed(provider?.trust.startsWith("untrusted") ? confirmed : false);
    setTestResult(undefined);
    setAutoMapResult(undefined);
  }

  function openKeyPicker() {
    primeDraft();
    setShowKeyPicker(true);
  }

  function chooseDraftProvider(provider: Provider) {
    setDraftProviderId(provider.id);
    setDraftApiKey("");
    setDraftDisplayName("");
    setDraftBaseUrl(provider.id === "custom" ? customBaseUrl : provider.baseUrl || "");
    setDraftModel("");
    setDraftModelAliases([]);
    setDraftRoleModels(emptyDraftRoleModels());
    setDraftAvailableModels([]);
    setDraftConfirmed(false);
    setTestResult(undefined);
    setAutoMapResult(undefined);
  }

  function applyDraftRoleModels(models: DraftRoleModels, availableModels = draftAvailableModels) {
    const available = uniqueModels(availableModels, Object.values(models));
    setDraftRoleModels(models);
    setDraftAvailableModels(available);
    setDraftModel(models.default);
    setDraftModelAliases(aliasesForDraftRoleModels(models));
  }

  function updateDraftRoleModel(role: SubscriptionRole, model: string) {
    applyDraftRoleModels({ ...draftRoleModels, [role]: model });
  }

  async function runNetworkQualityCheck() {
    if (networkCheckingRef.current) return;
    networkCheckingRef.current = true;
    setNetworkChecking(true);
    const commitNetworkResult = (next: WorkReport) => {
      laneStateRef.current = laneReducer(laneStateRef.current, {
        lane: "work",
        value: { probe: "network_quality", value: next },
      });
      setStatus((current) => ({
        ...current,
        network: next.deep
          ? {
            ...current.network,
            ...next.deep,
            ready: current.network.localReady
              && !current.network.daemonIoBlocked
              && next.deep.deepChecked
              && next.deep.sandboxEgressState === "ok",
          }
          : current.network,
      }));
      setWorkWarnings(next.warnings);
    };
    try {
      const result = await runNonGatingProbe<WorkReport>({
        lane: "work",
        key: "network_quality",
        timeoutMs: NETWORK_QUALITY_TIMEOUT_MS,
        circuit: networkQualityCircuitRef.current,
        task: () => isTauri
          ? invoke<WorkReport>("run_network_quality_check")
          : Promise.resolve({
          operation: "network_quality",
          ok: true,
          code: "work.network_quality.ok",
          deep: {
            deepChecked: true,
            sandboxUnixSocketState: "connected",
            sandboxSocksHandshakeState: "ok",
            sandboxEgressState: "ok",
            sandboxEgressFailureStage: "none",
            sandboxEgressTarget: "pypi.org",
            sandboxEgressHttpStatus: 200,
          },
          warnings: browserPreviewStatus.warnings,
        }),
        classifyValue: (value) => value.ok
          ? undefined
          : classifyNonGatingFailure(
            "work",
            "network_quality",
            `${value.code}: ${value.warnings.join("；")}`,
          ),
      });
      if (!result.ok) {
        laneStateRef.current = laneReducer(laneStateRef.current, {
          lane: "work",
          value: { probe: "network_quality", errorCode: result.code },
        });
        setWorkWarnings([]);
        setProbeNotice({
          source: "work.network_quality",
          code: result.code,
          message: result.message,
          muted: nonGatingFailureIsMuted(result.code, result.timedOut, result.skipped),
        });
        return;
      }
      commitNetworkResult(result.value);
      if (!result.value.ok) {
        const muted = nonGatingFailureIsMuted(result.value.code);
        if (muted) setWorkWarnings([]);
        setProbeNotice({
          source: "work.network_quality",
          code: result.value.code,
          message: result.value.warnings.join("；") || "network quality probe reported a confirmed failure",
          muted,
        });
      } else {
        setProbeNotice((current) => current?.source === "work.network_quality" ? undefined : current);
      }
    } finally {
      networkCheckingRef.current = false;
      setNetworkChecking(false);
    }
  }

  async function runAction(command: "start_services" | "stop_services" | "restart_services" | "stop_legacy_windows_bridge") {
    if (command === "restart_services") return runRepairAction();
    if (allowActionBusyRef.current) return;
    updateAllowActionBusy(true);
    setError("");
    let actionFailed = false;
    try {
      await invoke<unknown>(command);
    } catch (reason) {
      actionFailed = true;
      setError(String(reason));
    } finally {
      try {
        await refreshAllow();
      } catch (reason) {
        if (!actionFailed) setError(String(reason));
      }
      updateAllowActionBusy(false);
      void refreshGrade();
    }
  }

  async function runRepairAction() {
    if (repairBusyRef.current) return;
    statusCommitEpoch.current += 1;
    repairBusyRef.current = true;
    setRepairBusy(true);
    setError("");
    let actionFailed = false;
    try {
      await invoke<unknown>("restart_services");
    } catch (reason) {
      actionFailed = true;
      setError(String(reason));
    } finally {
      try {
        await refreshAllow();
      } catch (reason) {
        if (!actionFailed) setError(String(reason));
      }
      repairBusyRef.current = false;
      setRepairBusy(false);
      void refreshGrade();
    }
  }

  async function applyDraftKey() {
    if (busyRef.current || networkCheckingRef.current) return;
    if (!draftProvider) return;
    if (status.restartBlocked) {
      setError("当前诊断不允许写入或切换 API Key；请先处理磁盘、WSL 或安装包问题。连接测试仍可使用。");
      return;
    }
    if (draftNeedsBaseUrl && !draftBaseUrl.trim()) {
      setError("请先填写自定义中转 Base URL。");
      return;
    }
    if (draftIsThirdParty && !draftConfirmed) {
      setError("中转服务需要先确认域名，避免 API Key 被误发到不熟悉的地址。");
      return;
    }
    if (draftProvider.id !== "claude" && !draftApiKey.trim()) {
      setError("请填写 API Key；已保存的 Key 可以直接从列表切换。");
      return;
    }
    if (draftAvailableModels.length > 0 && Object.values(draftRoleModels).some((model) => !model)) {
      setError("请为决策、视觉和日常三层都选择模型；三层可以使用同一个模型。");
      return;
    }

    if (!isTauri) {
      setError("");
      const id = `preview-${Date.now()}`;
      const entry: ApiKeyEntry = {
        id,
        providerId: draftProvider.id,
        label: previewLabelForProvider(apiKeys, draftProvider, draftDisplayName),
        baseUrl: draftBaseUrl,
        model: draftModel,
        customConfirmed: draftConfirmed,
        modelAliases: draftModelAliases,
        hasSecret: Boolean(draftApiKey.trim()),
        active: false,
      };
      setApiKeys((current) => [...current, entry]);
      setPendingApiKeyId(id);
      setShowKeyPicker(false);
      setDraftApiKey("");
      setDraftDisplayName("");
      setDraftRoleModels(emptyDraftRoleModels());
      setDraftAvailableModels([]);
      return;
    }

    if (!tryBeginMutation()) return;
    setError("");
    try {
      const saved = await invoke<LauncherSettings>("save_api_key", {
        selectedProviderId: draftProvider.id,
        apiKey: draftApiKey,
        displayName: draftDisplayName,
        customBaseUrl: draftBaseUrl,
        customConfirmed: draftConfirmed,
        model: draftModel,
        modelAliases: draftModelAliases,
      });
      const previousIds = new Set(apiKeys.map((entry) => entry.id));
      const added = (saved.apiKeys || []).find((entry) => !previousIds.has(entry.id));
      applyLauncherState(saved);
      setPendingApiKeyId(added?.id || saved.activeApiKeyId);
      setDraftApiKey("");
      setDraftDisplayName("");
      setDraftModelAliases([]);
      setDraftRoleModels(emptyDraftRoleModels());
      setDraftAvailableModels([]);
      setAutoMapResult(undefined);
      setShowKeyPicker(false);
    } catch (reason) {
      setError(String(reason));
    } finally {
      updateBusy(false);
    }
  }

  async function testDraftApiKey() {
    if (!draftProvider || draftProvider.id === "claude") return;
    if (draftNeedsBaseUrl && !draftBaseUrl.trim()) {
      setError("请先填写自定义中转 Base URL。");
      return;
    }
    if (draftIsThirdParty && !draftConfirmed) {
      setError("中转服务需要先确认域名后再测试，避免 API Key 发到错误地址。");
      return;
    }
    if (!draftApiKey.trim()) {
      setError("请先填写 API Key，再测试连通。");
      return;
    }

    if (!isTauri) {
      setError("");
      const preview: ApiKeyTestResult = {
        ok: true,
        providerId: draftProvider.id,
        baseUrl: draftBaseUrl,
        upstreamMode: draftProvider.protocol.includes("anthropic") ? "anthropic" : "openai",
        selectedModel: draftModel || "preview-model",
        reply: "OK",
        models: [draftModel || "preview-model"],
        message: "浏览器预览模式：真实测试请打开 Tauri 启动器。",
      };
      setTestResult(preview);
      const available = uniqueModels(preview.models, [preview.selectedModel]);
      applyDraftRoleModels(inferDraftRoleModels(available, preview.selectedModel), available);
      setAutoMapResult(undefined);
      return;
    }

    setTestingKey(true);
    setError("");
    setTestResult(undefined);
    try {
      const result = await invoke<ApiKeyTestResult>("test_api_key", {
        selectedProviderId: draftProvider.id,
        apiKey: draftApiKey,
        customBaseUrl: draftBaseUrl,
        customConfirmed: draftConfirmed,
        model: draftModel,
        prompt: testPrompt,
      });
      setTestResult(result);
      if (result.ok && result.selectedModel) {
        const available = uniqueModels(result.models, [result.selectedModel]);
        applyDraftRoleModels(inferDraftRoleModels(available, result.selectedModel), available);
        setAutoMapResult(undefined);
      }
      if (!result.ok) {
        setError(result.message);
      }
    } catch (reason) {
      setError(String(reason));
    } finally {
      setTestingKey(false);
    }
  }

  async function autoMapDraftApiKey() {
    if (!draftProvider || draftProvider.id === "claude") return;
    if (draftNeedsBaseUrl && !draftBaseUrl.trim()) {
      setError("请先填写自定义中转 Base URL。");
      return;
    }
    if (draftIsThirdParty && !draftConfirmed) {
      setError("中转服务需要先确认域名后再获取模型列表，避免 API Key 发到错误地址。");
      return;
    }
    if (!draftApiKey.trim()) {
      setError("请先填写 API Key，再获取模型列表。");
      return;
    }

    if (!isTauri) {
      setError("");
      const primaryModel = draftModel || "preview-pro-model";
      const fastModel = primaryModel.includes("fast") ? primaryModel : "preview-fast-model";
      const available = uniqueModels([primaryModel, "preview-vision-model", fastModel]);
      const roles = inferDraftRoleModels(available, primaryModel, fastModel);
      const aliases = aliasesForDraftRoleModels(roles);
      applyDraftRoleModels(roles, available);
      setAutoMapResult({
        ok: true,
        providerId: draftProvider.id,
        baseUrl: draftBaseUrl,
        upstreamMode: draftProvider.protocol.includes("anthropic") ? "anthropic" : "openai",
        primaryModel,
        fastModel,
        aliases,
        models: available,
        message: "浏览器预览模式：已生成模型列表示例；真实列表请打开 Tauri 启动器。",
      });
      return;
    }

    setAutoMappingKey(true);
    setError("");
    setAutoMapResult(undefined);
    try {
      const result = await invoke<ApiKeyAutoMapResult>("auto_map_api_key", {
        selectedProviderId: draftProvider.id,
        apiKey: draftApiKey,
        customBaseUrl: draftBaseUrl,
        customConfirmed: draftConfirmed,
        model: draftModel,
      });
      setAutoMapResult(result);
      const available = uniqueModels(result.models, [result.primaryModel, result.fastModel]);
      applyDraftRoleModels(inferDraftRoleModels(available, result.primaryModel, result.fastModel), available);
    } catch (reason) {
      setError(String(reason));
      setDraftModelAliases([]);
      setDraftRoleModels(emptyDraftRoleModels());
      setDraftAvailableModels([]);
    } finally {
      setAutoMappingKey(false);
    }
  }

  function autoMatchDraftModels() {
    if (draftAvailableModels.length === 0) {
      void autoMapDraftApiKey();
      return;
    }
    applyDraftRoleModels(
      inferDraftRoleModels(draftAvailableModels, draftModel, draftRoleModels.fast),
      draftAvailableModels,
    );
  }

  async function activateKey(apiKeyId: string) {
    if (busyRef.current || networkCheckingRef.current) return;
    if (apiKeyId === activeApiKeyId && !activeRole && !activeAggregateSchemeId) return;
    if (status.restartBlocked) {
      setError("当前诊断不允许切换 API Key；请先处理磁盘、WSL 或安装包问题。");
      return;
    }
    if (!isTauri) {
      const entry = apiKeys.find((item) => item.id === apiKeyId);
      if (!entry) return;
      setError("");
      setActiveApiKeyId(apiKeyId);
      setPendingApiKeyId(apiKeyId);
      setActiveProvider(entry.providerId);
      setActiveRole(undefined);
      setActiveAggregateSchemeId(undefined);
      setAccessMode("api");
      setApiKeys((current) => current.map((item) => ({ ...item, active: item.id === apiKeyId })));
      return;
    }
    if (!tryBeginMutation()) return;
    setError("");
    let applied = false;
    try {
      applyLauncherState(await invoke<LauncherSettings>("activate_api_key", { apiKeyId }));
      applied = true;
    } catch (reason) {
      setError(String(reason));
    } finally {
      updateBusy(false);
    }
    if (applied) await refresh();
  }

  function updateRoleSubscription(role: SubscriptionRole, apiKeyId: string) {
    const entry = apiKeys.find((item) => item.id === apiKeyId);
    setRoleBindings((current) => current.map((binding) => binding.role === role
      ? {
          ...binding,
          providerId: entry?.providerId || "",
          apiKeyId,
          model: suggestedRoleModel(entry, role),
        }
      : binding));
    setRoleMappingsDirty(true);
  }

  function updateRoleModel(role: SubscriptionRole, model: string) {
    setRoleBindings((current) => current.map((binding) => binding.role === role
      ? { ...binding, model }
      : binding));
    setRoleMappingsDirty(true);
  }

  async function saveRoleMappings() {
    if (busyRef.current || networkCheckingRef.current) return;
    if (roleBindings.some((binding) => !binding.apiKeyId || !binding.providerId || !binding.model)) {
      setError("请为决策、视觉和日常三个角色都选择订阅与模型；三个角色可以使用同一订阅。");
      return;
    }
    if (!isTauri) {
      setError("");
      setRoleMappingsDirty(false);
      setAggregateSchemes((current) => current.map((scheme) => scheme.id === selectedSchemeId
        ? { ...scheme, routes: roleBindings }
        : scheme));
      setActiveAggregateSchemeId(selectedSchemeId);
      setPendingSchemeId(selectedSchemeId);
      setActiveRole(undefined);
      setAccessMode("aggregate");
      return;
    }
    if (!tryBeginMutation()) return;
    setError("");
    let applied = false;
    try {
      applyLauncherState(await invoke<LauncherSettings>("save_and_activate_aggregate_scheme", {
        scheme: {
          id: selectedSchemeId,
          name: selectedSchemeId === "scheme-1" ? "方案一" : "方案二",
          routes: roleBindings,
        },
      }));
      applied = true;
    } catch (reason) {
      setError(String(reason));
    } finally {
      updateBusy(false);
    }
    if (applied) await refresh();
  }

  function loadAggregateSchemeDraft(schemeId: AggregateSchemeId) {
    const scheme = aggregateSchemes.find((item) => item.id === schemeId);
    if (!scheme) return false;
    setSelectedSchemeId(schemeId);
    setPendingSchemeId(schemeId);
    const draftSettings: LauncherSettings = {
      selectedProviderId: activeProvider,
      customBaseUrl,
      customConfirmed,
      activeApiKeyId,
      apiKeys,
      activeRole,
      roleBindings: scheme.routes,
      activeAggregateSchemeId,
      aggregateSchemes,
    };
    setRoleBindings(roleDraftsFromSettings(draftSettings, scheme.routes));
    setRoleMappingsDirty(
      scheme.routes.length < roleDefinitions.length
      && apiKeys.some((entry) => entry.hasSecret && modelsForApiKey(entry).length > 0),
    );
    const complete = scheme.routes.length === roleDefinitions.length
      && scheme.routes.every((route) => route.apiKeyId && route.model);
    setAccessMode("aggregate");
    setError(complete ? "" : "这套方案尚未配置完整，补齐三条路由后点击“保存并应用整套方案”。");
    return complete;
  }

  function preselectAggregateScheme(schemeId: AggregateSchemeId) {
    if (busy || status.restartBlocked) return;
    if (roleMappingsDirty && schemeId !== selectedSchemeId) {
      setError("当前方案有未应用修改，请先保存并应用，或取消修改后再预选另一套方案。");
      return;
    }
    loadAggregateSchemeDraft(schemeId);
  }

  async function confirmPendingAggregateScheme() {
    if (busyRef.current || networkCheckingRef.current || status.restartBlocked || roleMappingsDirty) return;
    if (pendingSchemeId === activeAggregateSchemeId) return;
    const scheme = aggregateSchemes.find((item) => item.id === pendingSchemeId);
    if (!scheme) {
      setError("没有找到待切换的聚合方案。");
      return;
    }
    const complete = scheme.routes.length === roleDefinitions.length
      && scheme.routes.every((route) => route.apiKeyId && route.providerId && route.model);
    if (!complete) {
      setError("这套方案尚未配置完整，请先补齐三条路由并保存应用。");
      return;
    }
    if (!isTauri) {
      setError("");
      setActiveAggregateSchemeId(pendingSchemeId);
      setAccessMode("aggregate");
      return;
    }
    if (!tryBeginMutation()) return;
    setError("");
    try {
      applyLauncherState(await invoke<LauncherSettings>("activate_aggregate_scheme", {
        schemeId: pendingSchemeId,
      }));
    } catch (reason) {
      setError(String(reason));
    } finally {
      updateBusy(false);
    }
  }

  function cancelPendingAggregateScheme() {
    if (busy) return;
    const target = activeAggregateSchemeId === "scheme-1" || activeAggregateSchemeId === "scheme-2"
      ? activeAggregateSchemeId
      : "scheme-1";
    loadAggregateSchemeDraft(target);
    setError("");
  }

  function switchAccessMode(mode: AccessMode) {
    setAccessMode(mode);
    setError("");
    if (mode === "api") {
      setPendingApiKeyId(activeApiKeyId);
    } else {
      setPendingSchemeId(selectedSchemeId);
    }
  }

  async function deleteKey(apiKeyId: string) {
    if (busyRef.current || networkCheckingRef.current) return false;
    if (!isTauri) {
      setError("");
      setApiKeys((current) => current.filter((item) => item.id !== apiKeyId));
      setRoleBindings((current) => current.map((binding) => binding.apiKeyId === apiKeyId
        ? { ...binding, providerId: "", apiKeyId: "", model: "" }
        : binding));
      setRoleMappingsDirty(true);
      return true;
    }
    if (!tryBeginMutation()) return false;
    setError("");
    let deleted = false;
    try {
      applyLauncherState(await invoke<LauncherSettings>("delete_api_key", { apiKeyId }));
      deleted = true;
    } catch (reason) {
      setError(String(reason));
    } finally {
      updateBusy(false);
    }
    return deleted;
  }

  function beginRenameKey(entry: ApiKeyEntry) {
    setDeleteConfirmApiKeyId(undefined);
    setRenameApiKeyId(entry.id);
    setRenameDisplayName(entry.label);
    setError("");
  }

  function cancelRenameKey() {
    setRenameApiKeyId(undefined);
    setRenameDisplayName("");
  }

  async function renameKey(apiKeyId: string) {
    if (busyRef.current || networkCheckingRef.current) return;
    const displayName = renameDisplayName.trim();
    if (!displayName) {
      setError("接入名称不能为空。");
      return;
    }
    if (!isTauri) {
      setError("");
      setApiKeys((current) => current.map((entry) => entry.id === apiKeyId
        ? { ...entry, label: displayName }
        : entry));
      cancelRenameKey();
      return;
    }
    if (!tryBeginMutation()) return;
    setError("");
    try {
      applyLauncherState(await invoke<LauncherSettings>("rename_api_key", {
        apiKeyId,
        displayName,
      }));
      cancelRenameKey();
    } catch (reason) {
      setError(String(reason));
    } finally {
      updateBusy(false);
    }
  }

  function beginDeleteKey(apiKeyId: string) {
    setRenameApiKeyId(undefined);
    setDeleteConfirmApiKeyId(apiKeyId);
  }

  function cancelDeleteKey(apiKeyId: string) {
    setDeleteConfirmApiKeyId(undefined);
    window.requestAnimationFrame(() => deleteButtonRefs.current.get(apiKeyId)?.focus());
  }

  async function confirmDeleteKey(apiKeyId: string) {
    if (await deleteKey(apiKeyId)) {
      setDeleteConfirmApiKeyId(undefined);
    }
  }

  async function primaryAction() {
    if (primaryButton.disabled || allowActionBusyRef.current) return;
    if (primaryButton.action === "stop_legacy_bridge") return runAction("stop_legacy_windows_bridge");
    if (primaryButton.action === "open") {
      updateAllowActionBusy(true);
      setError("");
      try {
        await invoke<void>("open_claude_science");
      } catch (reason) {
        // Show the login failure immediately. A full storage/network refresh
        // can take minutes on DrvFS and must not keep the Open button locked.
        setError(String(reason));
      } finally {
        updateAllowActionBusy(false);
      }
      return;
    }
    if (primaryButton.action === "install") {
      setError("runtime.install_required: 请先在解压目录运行体检 Skill：repair-approved.ps1 -PlanOnly；确认计划后再执行 -ApproveInstall -StartServices。");
      return;
    }
    return runAction("start_services");
  }

  async function openDashboard() {
    try {
      await invoke<void>("open_bridge_dashboard");
    } catch (reason) {
      setError(String(reason));
    }
  }

  function openMigrationAssistant() {
    setMigrationCopyState("");
    setShowMigrationAssistant(true);
  }

  async function copyMigrationPrompt() {
    try {
      await navigator.clipboard.writeText(migrationPrompt);
      setMigrationCopyState("Prompt 已复制，可以粘贴到 Codex。");
    } catch {
      setMigrationCopyState("自动复制失败，请在下方文本框中按 Ctrl+A、Ctrl+C 手动复制。");
    }
  }

  function preselectKey(apiKeyId: string) {
    if (busy || status.restartBlocked) return;
    setPendingApiKeyId(apiKeyId);
    setError("");
  }

  async function confirmPendingKey() {
    if (!pendingApiKeyId || pendingApiKeyId === activeApiKeyId) return;
    await activateKey(pendingApiKeyId);
  }

  function cancelPendingKey() {
    if (busy) return;
    setPendingApiKeyId(activeApiKeyId);
    setError("");
  }

  async function checkRuntimeUpdate() {
    if (runtimeCheckingRef.current) return;
    runtimeCheckingRef.current = true;
    setRuntimeChecking(true);
    setRuntimeError("");
    try {
      const result = await runNonGatingProbe<RuntimeUpdateStatus>({
        lane: "work",
        key: "runtime_update",
        timeoutMs: RUNTIME_UPDATE_TIMEOUT_MS,
        circuit: runtimeUpdateCircuitRef.current,
        task: () => isTauri
          ? invoke<RuntimeUpdateStatus>("get_runtime_update_status")
          : Promise.resolve(browserPreviewRuntimeStatus),
      });
      if (!result.ok) {
        laneStateRef.current = laneReducer(laneStateRef.current, {
          lane: "work",
          value: { probe: "runtime_update", errorCode: result.code },
        });
        const muted = nonGatingFailureIsMuted(result.code, result.timedOut, result.skipped);
        setProbeNotice({
          source: "work.runtime_update",
          code: result.code,
          message: result.message,
          muted,
        });
        if (!muted) {
          setRuntimeError(nonGatingFailurePresentation(`${result.code}: ${result.message}`, allowStatus));
        }
        return;
      }
      laneStateRef.current = laneReducer(laneStateRef.current, {
        lane: "work",
        value: { probe: "runtime_update", value: result.value },
      });
      setProbeNotice((current) => current?.source === "work.runtime_update" ? undefined : current);
      setRuntimeUpdate(result.value);
    } finally {
      runtimeCheckingRef.current = false;
      setRuntimeChecking(false);
    }
  }

  function openRuntimePrompt(mode: "upgrade" | "rollback") {
    if (mode === "upgrade" && !runtimeUpdate) {
      setRuntimeError("请先成功读取官方版本索引，再生成升级 Prompt。");
      return;
    }
    const next = runtimeUpdate || browserPreviewRuntimeStatus;
    setRuntimeUpdate(next);
    setRuntimePromptMode(mode);
    setRuntimeCopyState("");
  }

  async function copyRuntimePrompt() {
    try {
      await navigator.clipboard.writeText(runtimePrompt);
      setRuntimeCopyState("Prompt 已复制，可以交给本地 Codex。 ");
    } catch {
      setRuntimeCopyState("自动复制失败，请在文本框中按 Ctrl+A、Ctrl+C 手动复制。");
    }
  }

  function openBridgeEgressAssistant() {
    setBridgeEgressReport(undefined);
    setBridgeEgressError("");
    setBridgeEgressCopyState("");
    setBridgeEgressApplyError("");
    setBridgeEgressApplyResult(undefined);
    setShowBridgeEgressAssistant(true);
  }

  async function confirmBridgeEgressCheck() {
    if (bridgeEgressCheckingRef.current) return;
    bridgeEgressCheckingRef.current = true;
    setBridgeEgressChecking(true);
    setBridgeEgressError("");
    setBridgeEgressCopyState("");
    try {
      const result = await runNonGatingProbe<BridgeEgressReport>({
        lane: "work",
        key: "bridge_egress",
        timeoutMs: BRIDGE_EGRESS_TIMEOUT_MS,
        circuit: bridgeEgressCircuitRef.current,
        task: () => isTauri
          ? invoke<BridgeEgressReport>("run_bridge_egress_check", { confirmBillable: true })
          : Promise.resolve(browserPreviewBridgeEgressReport),
      });
      if (!result.ok) {
        laneStateRef.current = laneReducer(laneStateRef.current, {
          lane: "work",
          value: { probe: "bridge_egress", errorCode: result.code },
        });
        const muted = nonGatingFailureIsMuted(result.code, result.timedOut, result.skipped);
        setBridgeEgressError(nonGatingFailurePresentation(`${result.code}: ${result.message}`, allowStatus));
        setProbeNotice({
          source: "work.bridge_egress",
          code: result.code,
          message: result.message,
          muted,
        });
        return;
      }
      laneStateRef.current = laneReducer(laneStateRef.current, {
        lane: "work",
        value: { probe: "bridge_egress", value: result.value },
      });
      setBridgeEgressReport(result.value);
      setBridgeEgressApplyError("");
      setBridgeEgressApplyResult(undefined);
      if (result.value.ok) {
        setProbeNotice((current) => current?.source === "work.bridge_egress" ? undefined : current);
      } else {
        setProbeNotice({
          source: "work.bridge_egress",
          code: result.value.code,
          message: result.value.conclusion,
          muted: nonGatingFailureIsMuted(result.value.code),
        });
      }
    } finally {
      bridgeEgressCheckingRef.current = false;
      setBridgeEgressChecking(false);
    }
  }

  async function copyBridgeEgressPrompt() {
    try {
      await navigator.clipboard.writeText(bridgeEgressPrompt);
      setBridgeEgressCopyState("修复 Prompt 已复制，可以交给本地 Codex。");
    } catch {
      setBridgeEgressCopyState("自动复制失败，请在文本框中按 Ctrl+A、Ctrl+C 手动复制。");
    }
  }

  async function applyBridgeEgressFix() {
    if (!recommendedBridgeEgressCandidate || bridgeEgressApplyBusyRef.current) return;
    bridgeEgressApplyBusyRef.current = true;
    setBridgeEgressApplyBusy(true);
    setBridgeEgressApplyError("");
    setBridgeEgressApplyResult(undefined);
    try {
      const result = isTauri
        ? await invoke<BridgeEgressApplyResult>("apply_bridge_egress_fix", {
          candidateUrl: recommendedBridgeEgressCandidate.address,
        })
        : await Promise.resolve<BridgeEgressApplyResult>({
          operation: "bridge_egress_apply",
          ok: true,
          code: "work.bridge_egress.apply_ok",
          backupPath: "/home/user/.claude-science/proxy/config.json.bak-20260825-123456",
          beforeOutboundProxyUrl: bridgeEgressReport?.outboundProxyUrl || "",
          afterOutboundProxyUrl: recommendedBridgeEgressCandidate.address,
          afterProbe: {
            ...browserPreviewBridgeEgressReport,
            ok: true,
            code: "work.bridge_egress.ok",
            conclusion: "Bridge 出口修复后验证通过。",
            outboundProxyUrl: recommendedBridgeEgressCandidate.address,
            proxy: { state: "passed", code: "work.bridge_egress.proxy_ok", durationMs: 2 },
            models: { state: "passed", code: "work.bridge_egress.models_ok", httpStatus: 200, durationMs: 81 },
            request: { state: "passed", code: "work.bridge_egress.request_ok", httpStatus: 200, durationMs: 213 },
          },
        });
      setBridgeEgressApplyResult(result);
      if (!result.ok) {
        setBridgeEgressApplyError(`${result.code}: 出口修复未通过验证。`);
      }
    } catch (caught) {
      const rawMessage = caught instanceof Error ? caught.message : String(caught);
      const message = rawMessage.startsWith("work.bridge_egress.apply_")
        ? rawMessage
        : `work.bridge_egress.apply_failed: ${rawMessage}`;
      setBridgeEgressApplyError(message);
    } finally {
      bridgeEgressApplyBusyRef.current = false;
      setBridgeEgressApplyBusy(false);
    }
  }

  const bridgeDetail = status.bridgeHealthy
    ? (status.bridgePid ? `PID ${status.bridgePid}` : "健康")
    : status.bridgeRunning
      ? (status.bridgePid ? `PID ${status.bridgePid}，健康检查失败` : "服务/端口存在，健康检查失败")
      : "已停止";
  const claudeDetail = allowStatus.claudeRunning
    ? (allowStatus.claudePid ? `PID ${allowStatus.claudePid}` : "端口已监听")
    : allowStatus.listenerPresent
      ? "端口存在，受管身份或双端口拓扑待验证"
      : "已停止";
  const proxyStateLabel: Record<string, string> = {
    direct: "直连环境",
    reachable: "代理可达",
    unreachable: "代理端口失联",
    conflict: "代理配置冲突",
    invalid: "代理配置无效",
    unknown: "代理状态未知",
    not_running: "待启动",
  };
  const deepEgressLabel = status.network.deepChecked
    ? status.network.sandboxEgressState === "ok"
      ? ` · SOCKS 握手与实链路通过${status.network.sandboxEgressHttpStatus ? ` HTTP ${status.network.sandboxEgressHttpStatus}` : ""}`
      : status.network.daemonIoBlocked
        ? ` · 当前守护进程忙（${status.network.daemonWaitChannel || "I/O 等待"}）`
        : ["daemon_mount_io_busy", "daemon_busy"].includes(status.network.sandboxEgressState)
          ? " · 深检时遇到瞬时 I/O；不影响本地打开，恢复后可重试"
        : ` · ${status.network.sandboxEgressFailureStage} 阶段失败（${status.network.sandboxEgressState}）`
    : " · 可深度检测";
  const networkDetail = allowStatus.claudeRunning
    ? `${proxyStateLabel[status.network.proxyState] || status.network.proxyState} · ${status.network.sandboxForwarderCount}/${status.network.sandboxForwarderExpectedCount} 组 HTTP/SOCKS 沙盒出口（${status.network.sandboxForwarderTopologyState}） · ${status.network.sandboxProbeRole}/${status.network.sandboxProbeTransport.toUpperCase()} 探针${deepEgressLabel}`
    : "Claude Science 启动后检查";
  const networkOk = status.network.ready
    && (!status.network.deepChecked || status.network.sandboxEgressState === "ok");
  const storageDetail = status.wslStoragePath
    ? `${status.wslStoragePath}${typeof status.wslStorageFreeGb === "number" ? ` · 宿主盘剩余 ${status.wslStorageFreeGb.toFixed(1)} GB` : ""}${typeof status.wslRootFreeGb === "number" ? ` · Linux 剩余 ${status.wslRootFreeGb.toFixed(1)} GB` : ""}`
    : "未定位 WSL 虚拟磁盘";

  const screenReady = canOpenClaude;
  const screenStarting = allowActionBusy && !screenReady && primaryButton.action === "start";
  const screenTitle = screenReady
    ? "Claude Science 已就绪"
    : screenStarting
      ? STARTUP_CAPTIONS[startupCaptionIndex]
      : "待机";
  const screenDetail = screenReady
    ? "8765 · 受管运行时 · Bridge 0.1.7"
    : screenStarting
      ? "正在准备唯一受管运行时"
      : "按「启动」开始";

  const screenCandidates = useMemo<ScreenCandidate[]>(() => {
    const rows: ScreenCandidate[] = [
      {
        key: "WSL2",
        value: allowStatus.wslInstalled
          ? `${allowStatus.distro || "默认发行版"} 就绪`
          : "未检测到可用发行版",
        tone: allowStatus.wslInstalled ? "normal" : "warning",
      },
      {
        key: "运行时",
        value: status.runtimeReady ? "受管运行时已准备" : "需要体检或安装",
        tone: status.runtimeReady ? "normal" : "warning",
      },
      {
        key: "Bridge",
        value: status.bridgeHealthy ? `${bridgeDetail} · 身份已验证` : bridgeDetail,
        tone: status.bridgeHealthy ? "normal" : "warning",
      },
      {
        key: "端口",
        value: allowStatus.claudeRunning
          ? `8765 / 8766${allowStatus.claudePid ? ` · PID ${allowStatus.claudePid}` : ""}`
          : claudeDetail,
        tone: allowStatus.claudeRunning ? "normal" : "warning",
      },
      {
        key: "沙盒",
        value: `${status.network.sandboxForwarderCount}/${status.network.sandboxForwarderExpectedCount} 组 HTTP/SOCKS · ${status.network.sandboxForwarderTopologyState}`,
        tone: status.network.sandboxForwarderCount === status.network.sandboxForwarderExpectedCount
          ? "normal"
          : "warning",
      },
      {
        key: "出口",
        value: networkDetail,
        tone: networkOk ? "normal" : "warning",
      },
      {
        key: "当前 Key",
        value: activeAggregateSchemeId
          ? `${activeAggregateSchemeId === "scheme-1" ? "方案一" : "方案二"} · 三个 Key 分工`
          : activeKeyEntry
            ? `${activeKeyEntry.label} · ${activeKeyProvider?.name || activeKeyEntry.providerId}`
            : "尚未添加接入",
        tone: activeAggregateSchemeId || activeKeyEntry ? "normal" : "warning",
      },
      {
        key: "存储",
        value: storageDetail,
        tone: status.storageWarning ? "warning" : "normal",
      },
    ];

    const addFault = (key: string, value: string, muted = false) => {
      if (!value) return;
      if (screenReady && !muted) {
        rows.push({ key, value, tone: "fault" });
        rows.push({ key: "主控", value: "仍可打开 Claude Science", tone: "safe" });
      } else {
        rows.push({ key, value, tone: "warning" });
      }
    };
    if (probeNotice) addFault("探针", screenMessage(probeNotice.message), probeNotice.muted);
    status.warnings.forEach((warning) => rows.push({ key: "诊断", value: screenMessage(warning), tone: "warning" }));
    workWarnings.forEach((warning) => rows.push({ key: "车间", value: screenMessage(warning), tone: "warning" }));
    if (error) addFault("错误", screenMessage(error));
    return rows;
  }, [
    activeAggregateSchemeId,
    activeKeyEntry,
    activeKeyProvider,
    allowStatus.claudePid,
    allowStatus.claudeRunning,
    allowStatus.distro,
    allowStatus.wslInstalled,
    bridgeDetail,
    claudeDetail,
    error,
    networkDetail,
    networkOk,
    probeNotice,
    screenReady,
    status.bridgeHealthy,
    status.network.sandboxForwarderCount,
    status.network.sandboxForwarderExpectedCount,
    status.network.sandboxForwarderTopologyState,
    status.runtimeReady,
    status.storageWarning,
    status.warnings,
    storageDetail,
    workWarnings,
  ]);

  useEffect(() => {
    if (!screenStarting) {
      setStartupCaptionIndex(0);
      return;
    }
    const timer = window.setInterval(() => {
      setStartupCaptionIndex((current) => Math.min(current + 1, STARTUP_CAPTIONS.length - 1));
    }, SCREEN_LINE_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [screenStarting]);

  useEffect(() => {
    if (skin !== "console" || !screenReady || screenPaused || screenHoverPaused || screenCandidates.length === 0) return;
    const appendNext = () => {
      const index = screenCandidateIndexRef.current % screenCandidates.length;
      const candidate = screenCandidates[index];
      const selected = [candidate];
      let consumed = 1;
      const next = screenCandidates[(index + 1) % screenCandidates.length];
      if (candidate.tone === "fault" && next?.tone === "safe") {
        selected.push(next);
        consumed = 2;
      }
      screenCandidateIndexRef.current = (index + consumed) % screenCandidates.length;
      const time = screenTime();
      setScreenLines((current) => [
        ...current,
        ...selected.map((item) => ({
          ...item,
          id: ++screenLineIdRef.current,
          time,
          tone: item.tone || "normal",
        })),
      ].slice(-SCREEN_LINE_LIMIT));
    };
    appendNext();
    const timer = window.setInterval(appendNext, SCREEN_LINE_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [screenCandidates, screenHoverPaused, screenPaused, screenReady, skin]);

  useEffect(() => {
    if (screenPaused || screenHoverPaused) return;
    const log = screenLogRef.current;
    if (log) log.scrollTop = log.scrollHeight;
  }, [screenHoverPaused, screenLines, screenPaused]);

  return (
    <main className="app-shell" data-skin={skin}>
      {!skinPreferenceResolved && (
        <div className="skin-loading-backdrop" role="status" aria-live="polite">
          <div><span className="nameplate-led starting" aria-hidden="true" />正在读取界面设置…</div>
        </div>
      )}
      {showSkinChooser && (
        <div className="skin-choice-backdrop" role="presentation">
          <section
            className="skin-choice-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="skin-choice-title"
          >
            <div className="skin-choice-head">
              <div>
                <span className="eyebrow">CSA Launcher</span>
                <h2 id="skin-choice-title">选择界面外观</h2>
                <p>三套外观使用同一组功能；选择会保存在启动器设置中。</p>
              </div>
              {!skinChoiceRequired && (
                <button className="quiet-button" onClick={() => setShowSkinChooser(false)} disabled={skinSaving}>关闭</button>
              )}
            </div>
            <div className="skin-choice-grid">
              <article className={`skin-option ${skin === "console" ? "selected" : ""}`}>
                <div className="skin-thumbnail console-thumbnail" aria-hidden="true">
                  <span className="thumb-screen">&gt;_ 就绪</span>
                  <span className="thumb-key" /><span className="thumb-key" /><span className="thumb-key" />
                </div>
                <h3>终端控制台</h3>
                <p>一块屏说状态，实体按键操作</p>
                <button aria-pressed={skin === "console"} onClick={() => void chooseSkin("console")} disabled={skinSaving || appearanceBlocked}>
                  使用终端控制台
                </button>
              </article>
              <article className={`skin-option ${skin === "classic" ? "selected" : ""}`}>
                <div className="skin-thumbnail classic-thumbnail" aria-hidden="true">
                  <span /><span /><span /><span />
                </div>
                <h3>经典面板</h3>
                <p>分区卡片，信息平铺</p>
                <button aria-pressed={skin === "classic"} onClick={() => void chooseSkin("classic")} disabled={skinSaving || appearanceBlocked}>
                  使用经典面板
                </button>
              </article>
              <article className={`skin-option ${skin === "dark" ? "selected" : ""}`}>
                <div className="skin-thumbnail dark-thumbnail" aria-hidden="true">
                  <span /><span /><span /><span />
                </div>
                <h3>深色面板</h3>
                <p>低亮环境使用，结构和功能不变</p>
                <button aria-pressed={skin === "dark"} onClick={() => void chooseSkin("dark")} disabled={skinSaving || appearanceBlocked}>
                  使用深色面板
                </button>
              </article>
            </div>
            {skinError && <p className="skin-choice-error" role="alert">{skinError}</p>}
          </section>
        </div>
      )}

      {showHelp && <HelpDialog version={APP_VERSION} onClose={closeHelp} />}

      <div
        className="app-content"
        aria-hidden={!skinPreferenceResolved || showSkinChooser || showHelp}
        inert={!skinPreferenceResolved || showSkinChooser || showHelp}
      >

      <header className="topbar nameplate">
        <span className={`nameplate-led ${screenReady ? "ready" : screenStarting ? "starting" : "standby"}`} aria-hidden="true" />
        <div className="brand-mark">CSA</div>
        <div className="brand-copy">
          <div className="brand-title-row">
            <h1>CSA - Claude Science Assistant</h1>
            <span className="app-version" aria-label={`启动器版本 ${APP_VERSION}`}>{APP_VERSION}</span>
          </div>
          <p>三模型聚合，一个安全启动入口</p>
        </div>
        <div className="topbar-actions">
          <button className="quiet-button" onClick={refresh} disabled={mutationBusy}>刷新状态</button>
        </div>
      </header>

      <section className={`hero state-${status.state}`}>
        <div className="status-orb"><span /></div>
        <div className="hero-copy">
          <span className="eyebrow">系统状态</span>
          <h2>{skin === "console" ? screenTitle : summary.title}<span className="screen-cursor" aria-hidden="true">▮</span></h2>
          <p>{skin === "console" ? screenDetail : summary.detail}</p>
          {screenStarting && (
            <div className="startup-progress" role="progressbar" aria-label="启动进度" aria-valuemin={0} aria-valuemax={5} aria-valuenow={startupCaptionIndex + 1}>
              <span style={{ width: `${((startupCaptionIndex + 1) / STARTUP_CAPTIONS.length) * 100}%` }} />
            </div>
          )}
          {screenReady && <>
            <div className="screen-readout-head">
              <span>实时读数 · 最近 {SCREEN_LINE_LIMIT} 行</span>
              <button type="button" onClick={() => setScreenPaused((current) => !current)} aria-pressed={screenPaused}>
                {screenPaused ? "继续" : "暂停"}
              </button>
            </div>
            <div
              className="screen-readout"
              role="log"
              aria-live="polite"
              aria-relevant="additions"
              ref={screenLogRef}
              onMouseEnter={() => setScreenHoverPaused(true)}
              onMouseLeave={() => setScreenHoverPaused(false)}
            >
              {screenLines.length === 0 && <div className="screen-line tone-normal"><span>--:--:--</span><strong>系统</strong><em>等待状态读数…</em></div>}
              {screenLines.map((line) => (
                <div className={`screen-line tone-${line.tone}`} key={line.id}>
                  <span>{line.time}</span>
                  <strong>{line.key}</strong>
                  <em>{line.value}</em>
                </div>
              ))}
            </div>
            {(screenPaused || screenHoverPaused) && <small className="screen-pause-state">读数已暂停，完整诊断仍保留在下方。</small>}
          </>}
        </div>
      </section>

      <section className="control-deck" aria-label="主控与功能键">
        <div className="primary-control">
          <span>主控</span>
          <button
            className="primary-button"
            onClick={primaryAction}
            disabled={primaryButton.disabled}
          >
            {primaryButton.label}
          </button>
        </div>
        <div className="function-keys">
          <button type="button" onClick={runNetworkQualityCheck} disabled={networkChecking || !allowStatus.claudeRunning}>
            <span>F1</span><strong>{networkChecking ? "检测中…" : "深度检测"}</strong>
          </button>
          <button type="button" onClick={openBridgeEgressAssistant} disabled={bridgeEgressChecking}>
            <span>F2</span><strong>{bridgeEgressChecking ? "体检中…" : "能力体检"}</strong>
          </button>
          <button
            type="button"
            onClick={() => runAction("restart_services")}
            disabled={repairBusy || !allowStatus.wslInstalled || status.restartBlocked}
            title="备份配置、收窄持久 DrvFS 写授权，并重启受管 Bridge 与 Claude Science"
          >
            <span>F3</span><strong>{repairBusy ? "修复中…" : "修复并重启"}</strong>
          </button>
        </div>
      </section>

      <details className="diagnostics-drawer" open={skin === "console" ? undefined : true}>
        <summary>
          <span>完整诊断与维护</span>
          <small>环境状态、错误详情、存储建议与运行时更新</small>
        </summary>
        <div className="diagnostics-content">
      <section className={`health-panel ${healthCollapsed ? "collapsed" : ""}`} aria-label="环境检查">
        <div className="health-panel-head">
          <div>
            <strong>环境状态</strong>
            <small>{healthCollapsed ? "7 项状态已收起" : "7 项状态（含沙盒真实出口）"}</small>
          </div>
          <button
            type="button"
            aria-expanded={!healthCollapsed}
            aria-controls="health-status-grid"
            onClick={() => {
              const next = !healthCollapsed;
              setHealthCollapsed(next);
              rememberHealthCollapsed(next);
            }}
          >
            {healthCollapsed ? "展开" : "收起"}
          </button>
        </div>
        {!healthCollapsed && (
          <div className="health-grid" id="health-status-grid">
            <HealthItem label="WSL2" ok={allowStatus.wslInstalled} detail={allowStatus.distro || "未检测到"} />
            <HealthItem label="运行时" ok={status.runtimeReady} detail={status.runtimeReady ? "已准备" : "需体检/修复"} />
            <HealthItem label="Bridge" ok={status.bridgeHealthy} detail={bridgeDetail} />
            <HealthItem label="Claude Science" ok={allowStatus.claudeRunning} detail={claudeDetail} />
            <HealthItem
              label="沙盒 / API 出口"
              ok={networkOk}
              detail={networkDetail}
              actionLabel={networkChecking ? "检测中…" : "深度检测"}
              onAction={runNetworkQualityCheck}
              actionDisabled={busy || networkChecking || !allowStatus.claudeRunning}
              secondaryActionLabel={bridgeEgressChecking ? "体检中…" : "能力体检"}
              onSecondaryAction={openBridgeEgressAssistant}
              secondaryActionDisabled={bridgeEgressChecking}
            />
            <HealthItem
              label="WSL 存储"
              ok={!status.storageWarning}
              detail={storageDetail}
              actionLabel={migrationRecommendation.actionLabel}
              onAction={openMigrationAssistant}
            />
            <HealthItem
              label="当前接入"
              ok={Boolean(activeAggregateSchemeId || activeKeyEntry)}
              detail={activeAggregateSchemeId
                ? `${activeAggregateSchemeId === "scheme-1" ? "方案一" : "方案二"} · 三模型聚合`
                : activeKeyEntry?.label || "未添加"}
            />
          </div>
        )}
      </section>

      {probeNotice && (
        <section className={`notice ${probeNotice.muted ? "probe-muted" : "probe-failure"}`} role="status">
          <strong>{probeNotice.muted ? "检测暂不可用" : "检测到非阻断故障"}</strong>
          <p>{nonGatingFailurePresentation(`${probeNotice.code}: ${probeNotice.message}`, allowStatus)}</p>
        </section>
      )}

      {(error || status.warnings.length > 0 || workWarnings.length > 0) && (
        <section className="notice" role="alert">
          <strong>诊断信息</strong>
          {error && <p>{error}</p>}
          {status.warnings.map((warning) => <p key={warning}>{warning}</p>)}
          {workWarnings.map((warning) => <p key={`work:${warning}`}>{warning}</p>)}
          {canOpenClaude && (status.warnings.length > 0 || workWarnings.length > 0) && (
            <p>上述诊断属于 GRADE/WORK，仍可打开 Claude Science</p>
          )}
          {allowStatus.windowsBridgePid && (
            <button className="notice-action" onClick={() => runAction("stop_legacy_windows_bridge")} disabled={allowActionBusy}>
              停止旧 Windows Bridge（PID {allowStatus.windowsBridgePid}）
            </button>
          )}
          {status.storageWarning && (
            <button className="notice-action" onClick={openMigrationAssistant} disabled={busy}>
              {migrationRecommendation.actionLabel}：交给 Codex
            </button>
          )}
        </section>
      )}

      <section className="runtime-update-panel" aria-label="Claude Science 运行时更新">
        <div className="runtime-update-copy">
          <span className="eyebrow">Runtime Update</span>
          <h2>Claude Science {runtimeUpdate?.bundledVersion || "0.1.25"}</h2>
          <p>{runtimeUpdate
            ? `官方 stable ${runtimeUpdate.stable.version} · latest ${runtimeUpdate.latest.version}`
            : "CSA 已验证版 0.1.25 · 可读取官方索引检查新版本"}</p>
          {runtimeError && <small className="runtime-update-error">{runtimeError}</small>}
        </div>
        <div className="runtime-update-state">
          <strong>{runtimeUpdate?.updateAvailable ? "发现官方新版本" : runtimeUpdate ? "已是 CSA 推荐版" : "尚未检查"}</strong>
          <small>{runtimeUpdate?.note || "检查不会安装或替换运行时"}</small>
        </div>
        <div className="runtime-update-actions">
          <button onClick={checkRuntimeUpdate} disabled={runtimeChecking}>{runtimeChecking ? "检查中…" : "检查更新"}</button>
          <button onClick={() => openRuntimePrompt("upgrade")} disabled={!runtimeUpdate}>升级 Prompt</button>
          <button onClick={() => openRuntimePrompt("rollback")}>回退 Prompt</button>
        </div>
      </section>
        </div>
      </details>

      {showMigrationAssistant && (
        <div className="migration-backdrop" role="presentation" onMouseDown={(event) => {
          if (event.currentTarget === event.target) setShowMigrationAssistant(false);
        }}>
          <section className={`migration-dialog migration-${migrationRecommendation.kind}`} role="dialog" aria-modal="true" aria-labelledby="migration-dialog-title">
            <div className="migration-dialog-head">
              <div>
                <span className="eyebrow">WSL 存储辅助迁移</span>
                <h2 id="migration-dialog-title">{migrationRecommendation.title}</h2>
                <p>{migrationRecommendation.detail}</p>
              </div>
              <button className="quiet-button" onClick={() => setShowMigrationAssistant(false)}>关闭</button>
            </div>

            <div className="migration-facts">
              <div><span>发行版</span><strong>{allowStatus.distro || "未检测到"}</strong></div>
              <div><span>当前位置</span><strong>{status.wslStoragePath || "未定位 VHDX"}</strong></div>
              <div><span>宿主盘剩余</span><strong>{typeof status.wslStorageFreeGb === "number" ? `${status.wslStorageFreeGb.toFixed(1)} GB` : "未检测到"}</strong></div>
              <div><span>Linux 剩余</span><strong>{typeof status.wslRootFreeGb === "number" ? `${status.wslRootFreeGb.toFixed(1)} GB` : "未检测到"}</strong></div>
            </div>

            <div className="migration-reasons">
              <strong>为什么出现这个建议</strong>
              {migrationRecommendation.reasons.map((reason) => <p key={reason}>• {reason}</p>)}
            </div>

            <div className="migration-boundary">
              启动器只生成本机化 Prompt，不会停止 WSL、移动 VHDX、修改注册表或执行 unregister。迁移与 CSA 增量升级是两条独立流程。
            </div>

            <label className="migration-prompt-label" htmlFor="migration-prompt">
              复制下面内容给 Codex
            </label>
            <textarea id="migration-prompt" value={migrationPrompt} readOnly spellCheck={false} />
            <div className="migration-actions">
              {migrationCopyState && <span aria-live="polite">{migrationCopyState}</span>}
              <button className="primary-inline-button" onClick={copyMigrationPrompt}>复制 Prompt</button>
            </div>
          </section>
        </div>
      )}

      {runtimePromptMode && runtimeUpdate && (
        <div className="migration-backdrop" role="presentation" onMouseDown={(event) => {
          if (event.currentTarget === event.target) setRuntimePromptMode(undefined);
        }}>
          <section className="migration-dialog" role="dialog" aria-modal="true" aria-labelledby="runtime-prompt-title">
            <div className="migration-dialog-head">
              <div>
                <span className="eyebrow">Claude Science Runtime</span>
                <h2 id="runtime-prompt-title">{runtimePromptMode === "upgrade" ? "安全升级 Prompt" : "安全回退 Prompt"}</h2>
                <p>由本地 Agent 先做隔离验证；未经你批准，不会替换真实运行时。</p>
              </div>
              <button className="quiet-button" onClick={() => setRuntimePromptMode(undefined)}>关闭</button>
            </div>
            <label className="migration-prompt-label" htmlFor="runtime-prompt">复制下面内容给 Codex</label>
            <textarea id="runtime-prompt" value={runtimePrompt} readOnly spellCheck={false} />
            <div className="migration-actions">
              {runtimeCopyState && <span aria-live="polite">{runtimeCopyState}</span>}
              <button className="primary-inline-button" onClick={copyRuntimePrompt}>复制 Prompt</button>
            </div>
          </section>
        </div>
      )}

      {showBridgeEgressAssistant && (
        <div className="migration-backdrop" role="presentation" onMouseDown={(event) => {
          if (!bridgeEgressChecking && event.currentTarget === event.target) setShowBridgeEgressAssistant(false);
        }}>
          <section
            className="migration-dialog bridge-egress-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="bridge-egress-title"
            ref={bridgeEgressDialogRef}
            tabIndex={-1}
          >
            <div className="migration-dialog-head">
              <div>
                <span className="eyebrow">Bridge → 上游模型 API</span>
                <h2 id="bridge-egress-title">{bridgeEgressReport ? "能力体检结果" : "能力体检确认"}</h2>
                <p>{bridgeEgressReport
                  ? "五层结果均来自 Bridge 实际使用的出口；它属于 WORK 车道，不影响本地打开。"
                  : "这与不计费的沙盒深度检测是两条不同链路。只有你明确同意后，才会开始真实出口体检。"}</p>
              </div>
              <button
                className="quiet-button"
                onClick={() => setShowBridgeEgressAssistant(false)}
                disabled={bridgeEgressChecking}
              >关闭</button>
            </div>

            {!bridgeEgressReport ? (
              <>
                <div className="bridge-egress-consent">
                  <strong>本次可能产生极少量模型费用</strong>
                  <p>同意后，体检会先检查 Bridge /health、outbound proxy TCP 和 /v1/models；前三层通过时，会向当前模型发送一次真实请求，<code>max_tokens=1</code>。</p>
                  <p>若提前查到 10808 等死代理，探针会在真实请求前停止，并明确显示“本次未发送真实请求”。</p>
                </div>
                <div className="migration-boundary">
                  探针全程只读：不改 outbound_proxy_url，不改系统代理、VPN、DNS、hosts、证书或 443，也不会关闭 WSL、启动器或无关服务。
                </div>
                {bridgeEgressError && <div className="bridge-egress-error" role="alert">{bridgeEgressError}</div>}
                <div className="migration-actions">
                  <button className="secondary-button" onClick={() => setShowBridgeEgressAssistant(false)} disabled={bridgeEgressChecking}>取消</button>
                  <button className="primary-inline-button" onClick={confirmBridgeEgressCheck} disabled={bridgeEgressChecking}>
                    {bridgeEgressChecking ? "正在执行五层体检…" : "同意并开始体检（1 次真实请求）"}
                  </button>
                </div>
              </>
            ) : (
              <>
                <div className={`bridge-egress-summary ${bridgeEgressReport.ok ? "ok" : "fail"}`}>
                  <strong>{bridgeEgressReport.code}</strong>
                  <p>{bridgeEgressReport.conclusion}</p>
                  <small>真实请求：{bridgeEgressReport.billableRequestSent ? "已发送 1 次（max_tokens=1）" : "未发送"} · 无论结果如何，仍可打开 Claude Science</small>
                </div>
                <div className="bridge-egress-layers" aria-label="Bridge 出口五层结果">
                  {([
                    ["1 · /health", bridgeEgressReport.health],
                    ["2 · proxy TCP", bridgeEgressReport.proxy],
                    ["3 · /v1/models", bridgeEgressReport.models],
                    ["4 · /v1/messages", bridgeEgressReport.request],
                    ["5 · direct control", bridgeEgressReport.direct],
                  ] as Array<[string, BridgeEgressLayer]>).map(([label, layer]) => (
                    <div className={`bridge-egress-layer ${layer.state}`} key={label}>
                      <span>{label}</span>
                      <strong>{layer.state}</strong>
                      <code>{layer.code}</code>
                      <small>{typeof layer.httpStatus === "number" ? `HTTP ${layer.httpStatus} · ` : ""}{Math.max(0, Math.round(layer.durationMs))} ms</small>
                    </div>
                  ))}
                </div>
                {bridgeEgressReport.candidates && bridgeEgressReport.candidates.length > 0 && (
                  <details className="migration-boundary" open>
                    <summary>出口候选（当前上游可达优先，直连最后）</summary>
                    <div className="bridge-egress-layers" aria-label="Bridge 出口候选">
                      {bridgeEgressReport.candidates.map((candidate: BridgeEgressCandidate, index) => (
                        <div
                          className={`bridge-egress-layer bridge-egress-candidate ${candidate.upstream?.state ?? candidate.tcp?.state ?? "skipped"}`}
                          key={`${candidate.address || "direct"}-${index}`}
                        >
                          <span>{index + 1} · {candidate.source || "未知来源"}{candidate.processName ? ` / ${candidate.processName}` : ""}</span>
                          <strong>{candidate.recommended ? "推荐" : "候选"}</strong>
                          <code>{displayBridgeEgressCandidateAddress(candidate)}</code>
                          <small>
                            TCP {candidate.tcp?.code || "未探测"}<br />
                            上游 {candidate.upstream?.code || "未探测"}<br />
                            {candidate.reason || "未提供推荐理由"}
                          </small>
                        </div>
                      ))}
                    </div>
                  </details>
                )}
                {recommendedBridgeEgressCandidate && (
                  <details className="migration-boundary" open>
                    <summary>WORK · 出口修复（只有用户点击才写入）</summary>
                    <p>
                      before：<code>{bridgeEgressReport.outboundProxyConfigured
                        ? displayBridgeEgressAddress(bridgeEgressReport.outboundProxyUrl)
                        : "直连（outbound_proxy_url 未配置）"}</code>
                    </p>
                    <p>
                      after：<code>{displayBridgeEgressCandidateAddress(recommendedBridgeEgressCandidate)}</code>
                    </p>
                    <p>推荐理由：{recommendedBridgeEgressCandidate.reason || "该候选的当前上游探测通过。"}</p>
                    <div className="bridge-egress-consent">
                      <strong>应用后的验证会发送 1 次真实请求</strong>
                      <p>验证使用 <code>max_tokens=1</code>，可能产生极少量模型费用；点击下方按钮即表示同意本次写入与这 1 次验证请求。</p>
                    </div>
                    {bridgeEgressApplyResult && (
                      <div className={`bridge-egress-summary ${bridgeEgressApplyResult.ok ? "ok" : "fail"}`} aria-live="polite">
                        <strong>{bridgeEgressApplyResult.code}</strong>
                        <p>
                          探针 before：<code>{bridgeEgressReport.code}</code>
                          {" → "}
                          after：<code>{bridgeEgressApplyResult.afterProbe?.code || "未返回"}</code>
                        </p>
                        <small>
                          配置 before：{displayBridgeEgressAddress(bridgeEgressApplyResult.beforeOutboundProxyUrl)} · after：{displayBridgeEgressAddress(bridgeEgressApplyResult.afterOutboundProxyUrl)}
                          {bridgeEgressApplyResult.backupPath ? ` · 备份：${bridgeEgressApplyResult.backupPath}` : ""}
                        </small>
                      </div>
                    )}
                    {bridgeEgressApplyError && <div className="bridge-egress-error" role="alert">{bridgeEgressApplyError}</div>}
                    <div className="migration-actions">
                      {bridgeEgressApplyBusy && <span aria-live="polite">正在备份、局部更新并验证…</span>}
                      <button
                        className="primary-inline-button"
                        onClick={applyBridgeEgressFix}
                        disabled={bridgeEgressApplyBusy}
                      >应用此修复（会修改 Bridge 出口配置）</button>
                    </div>
                  </details>
                )}
                <div className="migration-boundary">
                  启动器不会自动修改代理配置。你可以复制 Prompt 做只读复核；只有上方 WORK 区域的显式应用按钮会备份并局部修改一个字段，验证失败时自动回滚。
                </div>
                <label className="migration-prompt-label" htmlFor="bridge-egress-prompt">复制下面内容给 Codex</label>
                <textarea id="bridge-egress-prompt" value={bridgeEgressPrompt} readOnly spellCheck={false} />
                <div className="migration-actions">
                  {bridgeEgressCopyState && <span aria-live="polite">{bridgeEgressCopyState}</span>}
                  <button className="secondary-button" onClick={() => {
                    setBridgeEgressReport(undefined);
                    setBridgeEgressError("");
                    setBridgeEgressCopyState("");
                    setBridgeEgressApplyError("");
                    setBridgeEgressApplyResult(undefined);
                  }}>重新检测（重新确认）</button>
                  <button className="primary-inline-button" onClick={copyBridgeEgressPrompt}>复制修复 Prompt</button>
                </div>
              </>
            )}
          </section>
        </div>
      )}

      <section className={`kit-section ${accessMode === "api" && apiSectionCollapsed ? "collapsed" : ""}`}>
        <div className="section-heading">
          <div>
            <span className="eyebrow">Model Access</span>
            <h2>接入模型</h2>
          </div>
          <div className="section-heading-actions">
            <p>{accessMode === "aggregate"
              ? `${activeAggregateSchemeId ? `${activeAggregateSchemeId === "scheme-1" ? "方案一" : "方案二"}已生效` : "尚未生效"} · 三个模型槽同时接入`
              : apiSectionCollapsed
                ? `${activeKeyEntry?.label || "未添加接入"} · 我的接入 ${apiKeys.length} 条`
                : `我的接入 ${apiKeys.length} 条 · 启用前会再次确认`}</p>
            {accessMode === "api" && <button
              type="button"
              aria-expanded={!apiSectionCollapsed}
              aria-controls="api-key-section-content"
              onClick={() => {
                const next = !apiSectionCollapsed;
                setApiSectionCollapsed(next);
                rememberApiSectionCollapsed(next);
              }}
            >
              {apiSectionCollapsed ? "展开" : "收起"}
            </button>}
          </div>
        </div>

        <div className="access-mode-switcher" aria-label="接入模式">
          <button
            className={accessMode === "api" ? "active" : ""}
            disabled={busy}
            onClick={() => void switchAccessMode("api")}
          >
            用一个 Key
          </button>
          <button
            className={accessMode === "aggregate" ? "active" : ""}
            disabled={busy}
            onClick={() => void switchAccessMode("aggregate")}
          >
            三个 Key 分工
          </button>
        </div>

        {accessMode === "api" && !apiSectionCollapsed && <div id="api-key-section-content">
        <div className="kit-layout">
          <aside className="kit-queue" aria-label="我的接入">
            <div className="kit-queue-head">
              <div>
                <strong>我的接入</strong>
                <small>{apiKeys.length} 条 · 一次只启用一条</small>
              </div>
              <button onClick={openKeyPicker} disabled={busy}>＋ 新增接入</button>
            </div>
            <div className="kit-queue-scroll">
              {apiKeys.length === 0 && <div className="key-empty">还没有接入，点击「＋ 新增接入」开始。</div>}
              {apiKeys.map((entry, index) => {
                const provider = providers.find((item) => item.id === entry.providerId);
                const active = entry.active;
                const pending = entry.id === pendingApiKeyId && !active;
                const referencedByActiveScheme = Boolean(
                  activeAggregateSchemeId
                  && aggregateSchemes
                    .find((scheme) => scheme.id === activeAggregateSchemeId)
                    ?.routes.some((route) => route.apiKeyId === entry.id),
                );
                const confirmingDelete = entry.id === deleteConfirmApiKeyId;
                const renaming = entry.id === renameApiKeyId;
                return (
                  <div
                    className={`kit-row ${active ? "active" : ""} ${pending ? "pending" : ""}`}
                    key={entry.id}
                  >
                    <div className="key-entry-main">
                      <span className="kit-index">{String(index + 1).padStart(2, "0")}</span>
                      <span className="kit-row-copy">
                        <strong>{entry.label}</strong>
                        <small>{provider?.name || entry.providerId} · {entry.hasSecret ? "••••••••" : "官方登录"} · 决策 {entry.model || "未选择"}</small>
                      </span>
                      {active
                        ? <span className="active-key-label">使用中</span>
                        : pending
                          ? <span className="pending-key-label">待生效</span>
                          : null}
                    </div>

                    {renaming ? (
                      <div className="rename-key-row">
                        <label>
                          <span>接入名称</span>
                          <input
                            ref={renameInputRef}
                            value={renameDisplayName}
                            maxLength={80}
                            onChange={(event) => setRenameDisplayName(event.currentTarget.value)}
                            onKeyDown={(event) => {
                              if (event.key === "Enter") void renameKey(entry.id);
                              if (event.key === "Escape") cancelRenameKey();
                            }}
                          />
                        </label>
                        <div><button onClick={cancelRenameKey} disabled={busy}>取消</button><button onClick={() => void renameKey(entry.id)} disabled={busy}>保存</button></div>
                      </div>
                    ) : confirmingDelete ? (
                      <div className="delete-confirm-row" role="group" aria-label={`删除 ${entry.label}`}>
                        <p>{deleteConfirmationText(entry.label)}</p>
                        <div>
                          <button ref={deleteCancelRef} onClick={() => cancelDeleteKey(entry.id)} disabled={busy}>取消</button>
                          <button className="danger-button" onClick={() => void confirmDeleteKey(entry.id)} disabled={busy}>确认删除</button>
                        </div>
                      </div>
                    ) : (
                      <div className="key-row-actions">
                        <button onClick={() => preselectKey(entry.id)} disabled={mutationBusy || active}>启用</button>
                        <button onClick={() => beginRenameKey(entry)} disabled={mutationBusy}>重命名</button>
                        <button
                          ref={(node) => {
                            if (node) deleteButtonRefs.current.set(entry.id, node);
                            else deleteButtonRefs.current.delete(entry.id);
                          }}
                          onClick={() => beginDeleteKey(entry.id)}
                          disabled={mutationBusy || active || referencedByActiveScheme}
                          title={referencedByActiveScheme ? "该接入正在被当前聚合方案使用" : undefined}
                        >删除</button>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
            <div className="key-switch-confirm" aria-live="polite">
              <span>{pendingApiKeyId && pendingApiKeyId !== activeApiKeyId
                ? `待切换：${apiKeys.find((entry) => entry.id === pendingApiKeyId)?.label || "已选供应商"}；确认后会重启 Bridge，并发送 1 次 max_tokens=1 真实验证请求`
                : "点击列表预选，确认后才会重启并生效"}</span>
              <div>
                <button onClick={cancelPendingKey} disabled={busy || pendingApiKeyId === activeApiKeyId}>取消</button>
                <button
                  className="confirm-switch-button"
                  onClick={() => void confirmPendingKey()}
                  disabled={mutationBusy || status.restartBlocked || !pendingApiKeyId || pendingApiKeyId === activeApiKeyId}
                >
                  {busy ? "切换中…" : "确认切换"}
                </button>
              </div>
            </div>
            <button className="add-kit-row" onClick={openKeyPicker} disabled={busy}>
              <span>＋</span>
              新增接入
            </button>
          </aside>
        </div>
        </div>}

        {accessMode === "aggregate" && <div className="role-mapping-panel">
          <div className="role-mapping-head">
            <div>
              <span className="eyebrow">Aggregate Scheme</span>
              <h3>三模型聚合方案</h3>
            </div>
            <div className="scheme-switcher" aria-label="切换聚合方案">
              {aggregateSchemes.map((scheme) => (
                <button
                  className={`${selectedSchemeId === scheme.id ? "active" : ""} ${activeAggregateSchemeId === scheme.id ? "applied" : ""}`}
                  key={scheme.id}
                  disabled={busy || status.restartBlocked}
                  onClick={() => preselectAggregateScheme(scheme.id)}
                >
                  {scheme.name}
                  {activeAggregateSchemeId === scheme.id
                    ? <span>已生效</span>
                    : pendingSchemeId === scheme.id
                      ? <span>待切换</span>
                      : null}
                </button>
              ))}
            </div>
          </div>

          <div className="key-switch-confirm scheme-switch-confirm" aria-live="polite">
            <span>{pendingSchemeId !== activeAggregateSchemeId
              ? `待切换：${pendingSchemeId === "scheme-1" ? "方案一" : "方案二"}；会重启 Bridge，并经受管 Bridge 对决策、视觉、日常三路各发送 1 次 max_tokens=1 真实验证请求（共 3 次，可能产生费用）`
              : "点击方案预选，确认后才会重启 Bridge；三条路由各验证 1 次，共 3 次 max_tokens=1 真实请求，可能产生费用"}</span>
            <div>
              <button onClick={cancelPendingAggregateScheme} disabled={busy}>取消</button>
              <button
                className="confirm-switch-button"
                onClick={() => void confirmPendingAggregateScheme()}
                disabled={mutationBusy || status.restartBlocked || roleMappingsDirty || !pendingSchemeComplete || pendingSchemeId === activeAggregateSchemeId}
              >
                {busy ? "正在验证线路…" : "确认切换"}
              </button>
            </div>
          </div>

          <div className="role-mapping-table">
            {roleDefinitions.map((definition) => {
              const binding = roleBindings.find((item) => item.role === definition.role) || {
                role: definition.role,
                providerId: "",
                apiKeyId: "",
                model: "",
              };
              const entry = apiKeys.find((item) => item.id === binding.apiKeyId);
              return (
                <div className="role-mapping-row" key={definition.role}>
                  <span className="role-name">
                    <strong>{definition.label}</strong>
                    <small>{definition.detail}</small>
                  </span>
                  <label>
                    <span>订阅</span>
                    <select
                      value={binding.apiKeyId}
                      disabled={busy || roleEligibleKeys.length === 0}
                      onChange={(event) => updateRoleSubscription(definition.role, event.currentTarget.value)}
                    >
                      <option value="">未选择</option>
                      {roleEligibleKeys.map((item) => (
                        <option value={item.id} key={item.id}>{item.label}</option>
                      ))}
                    </select>
                  </label>
                  <label>
                    <span>模型</span>
                    <select
                      value={binding.model}
                      disabled={busy || !entry}
                      onChange={(event) => updateRoleModel(definition.role, event.currentTarget.value)}
                    >
                      <option value="">未选择</option>
                      {modelsForApiKey(entry).map((model) => (
                        <option value={model} key={model}>{model}</option>
                      ))}
                    </select>
                  </label>
                </div>
              );
            })}
          </div>

          <div className="role-mapping-footer">
            <span>{roleMappingsDirty
              ? `${selectedSchemeId === "scheme-1" ? "方案一" : "方案二"}有未应用修改；应用后会经受管 Bridge 对三条路由各发送 1 次 max_tokens=1 真实验证请求（共 3 次，可能产生费用）`
              : activeAggregateSchemeId === selectedSchemeId
                ? "当前方案的三个模型槽已同时生效"
                : "当前方案尚未应用"}</span>
            <button
              className="primary-inline-button"
              onClick={saveRoleMappings}
              disabled={mutationBusy || !roleMappingsDirty || roleEligibleKeys.length === 0}
            >
              保存并应用整套方案
            </button>
          </div>
        </div>}

        {showKeyPicker && (
          <div className="kit-picker" role="dialog" aria-label="新增接入">
            <div className="kit-picker-head">
              <div>
                <span className="eyebrow">＋ 新增接入</span>
                <h3>四步完成一个模型接入</h3>
              </div>
              <button className="quiet-button" onClick={() => setShowKeyPicker(false)} disabled={busy}>关闭</button>
            </div>

            <div className="kit-picker-grid">
              <div className="template-list">
                <div className="add-step-heading"><span>①</span><strong>选供应商</strong></div>
                {providerGroups.map((group, groupIndex) => (
                  <div className="template-group" key={group.title}>
                    <h4>{group.title}</h4>
                    {group.providers.map((provider, providerIndex) => {
                      const order = providerGroups
                        .slice(0, groupIndex)
                        .reduce((count, item) => count + item.providers.length, 0) + providerIndex + 1;
                      return (
                        <button
                          className={`template-row ${draftProviderId === provider.id ? "selected" : ""}`}
                          key={provider.id}
                          onClick={() => chooseDraftProvider(provider)}
                        >
                          <span className="kit-index">{String(order).padStart(2, "0")}</span>
                          <span className="provider-icon">{providerInitial(provider)}</span>
                          <span className="template-copy">
                            <strong>{provider.name}</strong>
                            <small>{provider.meta}</small>
                          </span>
                          <span className={`trust-badge badge-${badgeClass[provider.badge]}`}>{provider.badge}</span>
                        </button>
                      );
                    })}
                    {group.tier === "custom" && <p className="relay-domain-hint">中转服务要先确认域名才能保存 Key。</p>}
                  </div>
                ))}
              </div>

              <div className="kit-form">
                <div className="kit-form-title">
                  <span className="provider-icon large">{providerInitial(draftProvider)}</span>
                  <div>
                    <strong>{draftProvider?.name}</strong>
                    <small>{draftProvider?.protocol}</small>
                  </div>
                </div>

                <div className="add-step-heading"><span>②</span><strong>粘贴 API Key</strong></div>

                {draftProvider?.id === "claude" ? (
                  <div className="relay-panel">
                    <strong>Claude 官方登录</strong>
                    <p>Claude 官方模式优先使用 Claude Science 自身登录态；如果后续要接 Claude API Key，我们再单独做一个安全存储方案。</p>
                  </div>
                ) : (
                  <label>
                    API Key
                    <input
                      type="password"
                      value={draftApiKey}
                      placeholder="请输入 API Key；保存后可从列表直接切换"
                      spellCheck={false}
                      autoComplete="off"
                      onChange={(event) => {
                        setDraftApiKey(event.currentTarget.value);
                        setTestResult(undefined);
                        setAutoMapResult(undefined);
                        setDraftModelAliases([]);
                        setDraftRoleModels(emptyDraftRoleModels());
                        setDraftAvailableModels([]);
                      }}
                    />
                  </label>
                )}

                {(draftProvider?.baseUrl || draftNeedsBaseUrl) && (
                  <label>
                    Base URL
                    <input
                      value={draftBaseUrl}
                      placeholder={draftNeedsBaseUrl ? "https://your-relay.example/v1" : draftProvider?.baseUrl}
                      spellCheck={false}
                      disabled={!draftNeedsBaseUrl}
                      onChange={(event) => {
                        setDraftBaseUrl(event.currentTarget.value);
                        setTestResult(undefined);
                        setAutoMapResult(undefined);
                        setDraftModelAliases([]);
                        setDraftRoleModels(emptyDraftRoleModels());
                        setDraftAvailableModels([]);
                      }}
                    />
                  </label>
                )}

                {draftProvider?.id !== "claude" && (
                  <div className="test-panel">
                    <div className="test-panel-head">
                      <div>
                        <strong>测试</strong>
                        <small>测试会读取模型列表，并可能对多个候选模型以两档输出预算发送多次真实请求；可能产生费用，请勿反复点击。</small>
                      </div>
                      <div className="test-panel-actions">
                        <button onClick={testDraftApiKey} disabled={busy || testingKey || autoMappingKey}>
                          {testingKey ? "测试中…" : "测试"}
                        </button>
                      </div>
                    </div>
                    <label>
                      测试消息
                      <input
                        value={testPrompt}
                        placeholder="Reply only: OK"
                        spellCheck={false}
                        onChange={(event) => setTestPrompt(event.currentTarget.value)}
                      />
                    </label>
                    {testResult && (
                      <div className={`test-result ${testResult.ok ? "ok" : "fail"}`}>
                        <strong>{testResult.ok ? "连通成功" : "连通失败"}</strong>
                        <p>{testResult.message}</p>
                        {testResult.selectedModel && <p>可用模型：{testResult.selectedModel}</p>}
                        {testResult.reply && <p>模型回复：{testResult.reply}</p>}
                      </div>
                    )}
                    {autoMapResult && (
                      <div className="mapping-result">
                        <strong>模型列表已读取</strong>
                        <p>{autoMapResult.message}</p>
                      </div>
                    )}
                  </div>
                )}

                <div className="add-step-heading model-step-heading"><span>③</span><strong>三个角色用哪个模型</strong></div>
                <label>
                  决策模型（可手动填写）
                  <input
                    value={draftModel}
                    placeholder="建议先测试或重新获取模型"
                    spellCheck={false}
                    onChange={(event) => {
                      const value = event.currentTarget.value;
                      setDraftModel(value);
                      setTestResult(undefined);
                      setAutoMapResult(undefined);
                      setDraftModelAliases([]);
                      setDraftRoleModels({ default: value, vision: "", fast: "" });
                    }}
                  />
                </label>
                <div className="model-step-actions">
                  <button onClick={autoMatchDraftModels} disabled={busy || testingKey || autoMappingKey || draftProvider?.id === "claude"}>
                    自动匹配
                  </button>
                  <button onClick={autoMapDraftApiKey} disabled={busy || testingKey || autoMappingKey || draftProvider?.id === "claude"}>
                    {autoMappingKey ? "获取中…" : "重新获取模型"}
                  </button>
                </div>
                <p className="form-help">测试通过后会自动填好，一般不用改。</p>
                <div className="draft-role-editor" aria-label="三个角色用哪个模型">
                  {roleDefinitions.map((definition) => (
                    <label className="draft-role-row" key={definition.role}>
                      <span className="draft-role-name">
                        <strong>{definition.label}</strong>
                        <small>{definition.detail}</small>
                      </span>
                      <select
                        value={draftRoleModels[definition.role]}
                        disabled={draftModelOptions.length === 0}
                        onChange={(event) => updateDraftRoleModel(definition.role, event.currentTarget.value)}
                      >
                        <option value="">选择模型</option>
                        {draftModelOptions.map((model) => (
                          <option value={model} key={model}>{model}</option>
                        ))}
                      </select>
                    </label>
                  ))}
                </div>

                <div className="add-step-heading"><span>④</span><strong>给它起个名字</strong></div>
                <label>
                  接入名称
                  <input
                    value={draftDisplayName}
                    maxLength={80}
                    placeholder={draftProvider?.id === "custom" ? "可留空；自动使用日期与序号" : draftProvider?.name || "可留空"}
                    spellCheck={false}
                    onChange={(event) => setDraftDisplayName(event.currentTarget.value)}
                  />
                </label>
                <p className="form-help">留空会自动取名。以后在「我的接入」里按这个名字找它。</p>

                {draftIsThirdParty && (
                  <label className="confirm-row">
                    <input
                      type="checkbox"
                      checked={draftConfirmed}
                      onChange={(event) => {
                        setDraftConfirmed(event.currentTarget.checked);
                        setTestResult(undefined);
                        setAutoMapResult(undefined);
                        setDraftModelAliases([]);
                        setDraftRoleModels(emptyDraftRoleModels());
                        setDraftAvailableModels([]);
                      }}
                    />
                    我已确认该中转服务域名，API Key 只发送到该地址。
                  </label>
                )}

                <div className="form-actions">
                  <div className="submit-copy">
                    <strong>切换只重启 Bridge，不会关掉 Claude Science。失败会自动切回原来的。</strong>
                    <small>先保存到列表，再显示费用确认；只有再次确认才会启用并发送 1 次 max_tokens=1 验证请求。</small>
                  </div>
                  <button
                    className="primary-inline-button"
                    onClick={applyDraftKey}
                    disabled={mutationBusy || testingKey || autoMappingKey || status.restartBlocked}
                    title="先保存到列表，再进入启用确认"
                  >
                    {busy ? "正在保存…" : "保存并启用"}
                  </button>
                  <button onClick={() => setShowKeyPicker(false)} disabled={busy || testingKey || autoMappingKey}>取消</button>
                </div>
              </div>
            </div>
          </div>
        )}
      </section>

      <footer>
        <span>{allowStatus.linuxUser && allowStatus.distro ? `${allowStatus.linuxUser} · ${allowStatus.distro}` : "Windows 10/11 · WSL2"}</span>
        <div className="footer-actions">
          <button className="help-button" onClick={() => setShowHelp(true)}>？ 帮助</button>
          <button className="appearance-button" disabled={appearanceBlocked} onClick={() => {
            setSkinChoiceRequired(false);
            setSkinError("");
            setShowSkinChooser(true);
          }}>外观设置</button>
          <button onClick={openDashboard} disabled={busy || !status.bridgeHealthy}>配置面板</button>
          <button
            onClick={() => runAction("restart_services")}
            disabled={repairBusy || !allowStatus.wslInstalled || status.restartBlocked}
            title="备份配置、收窄持久 DrvFS 写授权，并重启受管 Bridge 与 Claude Science"
          >修复并重启</button>
          <button onClick={() => runAction("stop_services")} disabled={allowActionBusy || (!status.bridgeRunning && !allowStatus.claudeRunning)}>停止</button>
        </div>
      </footer>
      </div>
    </main>
  );
}

function HealthItem({
  label,
  ok,
  detail,
  actionLabel,
  onAction,
  actionDisabled,
  secondaryActionLabel,
  onSecondaryAction,
  secondaryActionDisabled,
}: {
  label: string;
  ok: boolean;
  detail: string;
  actionLabel?: string;
  onAction?: () => void;
  actionDisabled?: boolean;
  secondaryActionLabel?: string;
  onSecondaryAction?: () => void;
  secondaryActionDisabled?: boolean;
}) {
  return (
    <div className="health-item">
      <span className={`health-check ${ok ? "ok" : ""}`}>{ok ? "✓" : "—"}</span>
      <div className="health-item-copy"><strong>{label}</strong><small>{detail}</small></div>
      <div className="health-item-actions">
        {actionLabel && onAction && <button className="health-item-action" onClick={onAction} disabled={actionDisabled}>{actionLabel}</button>}
        {secondaryActionLabel && onSecondaryAction && (
          <button className="health-item-action" onClick={onSecondaryAction} disabled={secondaryActionDisabled}>{secondaryActionLabel}</button>
        )}
      </div>
    </div>
  );
}

export default App;
