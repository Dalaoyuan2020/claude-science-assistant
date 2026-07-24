import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import QRCode from "qrcode";
import { buildStorageMigrationPrompt, storageRecommendation } from "./storageMigration";
import "./App.css";

type SystemState = "loading" | "notInstalled" | "stopped" | "degraded" | "running" | "error";
type FeatureModule = "api-key" | "subagent" | "connect" | "research-os";

interface SystemStatus {
  state: SystemState;
  wslInstalled: boolean;
  distro?: string;
  linuxUser?: string;
  bridgeRunning: boolean;
  bridgePid?: number;
  claudeRunning: boolean;
  claudePid?: number;
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
  warnings: string[];
}

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

interface LauncherSettings {
  selectedProviderId: string;
  customBaseUrl: string;
  customConfirmed: boolean;
  activeApiKeyId?: string;
  apiKeys: ApiKeyEntry[];
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

interface ExternalAgentRunResult {
  ok: boolean;
  tool: string;
  exitCode?: number;
  durationMs: number;
  stdout: string;
  stderr: string;
  resultText?: string;
  sessionId?: string;
  resumeCommand?: string;
  message: string;
}

interface SubagentRequest {
  schemaVersion?: number;
  source?: string;
  taskKind?: string;
  title?: string;
  cwd?: string;
  note?: string;
  requestedAction?: string;
  approvalMode?: string;
  policyId?: string;
  createdAt?: string;
}

interface SubagentInboxItem {
  requestId: string;
  fileName: string;
  filePath: string;
  modifiedMs: number;
  request?: SubagentRequest;
  parseError?: string;
}

interface SubagentRunResult {
  runId: string;
  requestId: string;
  resultDir: string;
  resultJsonPath: string;
  agent: ExternalAgentRunResult;
}

interface SubagentRunHistoryItem {
  runId: string;
  kind: "run" | "continue" | string;
  requestId?: string;
  parentRunId?: string;
  resultDir: string;
  resultJsonPath: string;
  modifiedMs: number;
  agent: ExternalAgentRunResult;
}

interface ExternalSessionLaunchResult {
  sessionId: string;
  command: string;
  cwd: string;
  terminal: string;
  message: string;
}

interface ClaudeSessionMessage {
  id: string;
  sessionId: string;
  role: "user" | "assistant" | string;
  kind: string;
  content: string;
  createdAt?: string;
}

interface ClaudeSessionHistory {
  sessionId: string;
  filePath: string;
  modifiedMs: number;
  messages: ClaudeSessionMessage[];
  totalMessages: number;
  hasMore: boolean;
}

interface SkillFeedItem {
  id: string;
  repositoryId: string;
  name: string;
  description: string;
  relativePath: string;
  modifiedMs: number;
  isNew: boolean;
}

interface SkillRepository {
  id: string;
  source: string;
  localPath: string;
  createdAt: number;
  lastSyncedAt: number;
  lastCommit: string;
  skills: SkillFeedItem[];
}

interface ResearchOsState {
  repositories: SkillRepository[];
}

interface GatewayCounts {
  authorized: number;
  queued: number;
  claimed: number;
  replied: number;
  needsLocalApproval: number;
  failed: number;
}

interface ConnectBotSummary {
  id: "feishu" | "telegram" | string;
  configured: boolean;
  running: boolean;
  paired: boolean;
  detail: string;
  lastError: string;
  updatedAt: number;
}

interface ConnectRuntimeState {
  installed: boolean;
  running: boolean;
  mcpReady: boolean;
  mcpUrl: string;
  skillInstalled: boolean;
  legacyFeishuWebhook: boolean;
  defaultWorkspacePath: string;
  counts: GatewayCounts;
  feishu: ConnectBotSummary;
  telegram: ConnectBotSummary;
  browserExtension: BrowserExtensionState;
  capabilities: Record<string, boolean>;
  error: string;
}

interface BrowserExtensionState {
  status: "notInstalled" | "installed" | "paired" | "online" | "pageReady" | "error" | string;
  serverUrl: string;
  extensionPath: string;
  paired: boolean;
  online: boolean;
  pageReady: boolean;
  composerReady: boolean;
  tabId?: number;
  url: string;
  pageTitle: string;
  lastSeenAt: number;
  lastError: string;
}

interface BrowserExtensionInstallInfo {
  extensionPath: string;
  chromeExtensionsUrl: string;
  instructions: string[];
}

interface ConnectPairingCode {
  channel: "feishu" | "telegram" | "browserExtension" | string;
  code: string;
  expiresAt: number;
  launchUrl?: string;
}

interface FeishuRegistrationStart {
  deviceCode: string;
  verificationUrl: string;
  expiresAt: number;
  intervalSeconds: number;
}

interface FeishuRegistrationPoll {
  status: "pending" | "completed" | "failed" | string;
  detail: string;
  runtime?: ConnectRuntimeState;
}

interface ConnectRoute {
  routeKey: string;
  channel: string;
  accountId: string;
  senderId: string;
  conversationId: string;
  threadId: string;
  bindingId: string;
  workspacePath: string;
  nativeFrameId: string;
  pairedAt: number;
  lastMessageAt: number;
  pendingMessages: number;
}

interface ConnectHistoryMessage {
  messageId: string;
  channel: string;
  platformEventId: string;
  senderId: string;
  conversationId: string;
  threadId: string;
  bindingId: string;
  workspacePath: string;
  kind: string;
  text: string;
  attachments: ConnectAttachment[];
  replyTo: string;
  direction: "inbound" | "outbound" | string;
  status: string;
  lastError: string;
  createdAt: number;
  updatedAt: number;
}

interface ConnectAttachment {
  attachmentId: string;
  kind: string;
  mimeType: string;
  fileName: string;
  sizeBytes: number;
  sha256: string;
  state: string;
}

interface ConnectorSetup {
  name: string;
  url: string;
  transport: string;
  authorizationHeader: string;
}

interface ConnectLocalSendResult {
  route: ConnectRoute;
  deliveryMode: "browserExtension" | "activeInject" | "queueFallback";
  message: string;
}

interface ConnectDispatchResult {
  dispatched: boolean;
  messageId: string;
  channel: string;
  detail: string;
}

const emptyGatewayCounts: GatewayCounts = {
  authorized: 0,
  queued: 0,
  claimed: 0,
  replied: 0,
  needsLocalApproval: 0,
  failed: 0,
};

const initialConnectState: ConnectRuntimeState = {
  installed: false,
  running: false,
  mcpReady: false,
  mcpUrl: "http://127.0.0.1:9881/mcp",
  skillInstalled: false,
  legacyFeishuWebhook: false,
  defaultWorkspacePath: "",
  counts: emptyGatewayCounts,
  feishu: { id: "feishu", configured: false, running: false, paired: false, detail: "企业自建应用长连接", lastError: "", updatedAt: 0 },
  telegram: { id: "telegram", configured: false, running: false, paired: false, detail: "Bot API 长轮询", lastError: "", updatedAt: 0 },
  browserExtension: { status: "notInstalled", serverUrl: "http://127.0.0.1:9882", extensionPath: "", paired: false, online: false, pageReady: false, composerReady: false, url: "", pageTitle: "", lastSeenAt: 0, lastError: "" },
  capabilities: { mcpQueue: true, workspaceFiles: true, nativeInject: false, browserExtension: true },
  error: "",
};

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
    title: "聚合平台",
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
};

const initialStatus: SystemStatus = {
  state: "loading",
  wslInstalled: false,
  bridgeRunning: false,
  claudeRunning: false,
  bridgeHealthy: false,
  runtimeReady: false,
  sourceBinaryPresent: false,
  bridgeVenvPresent: false,
  storageWarning: false,
  storageBlocked: false,
  restartBlocked: false,
  warnings: [],
};

const browserPreviewStatus: SystemStatus = {
  state: "notInstalled",
  wslInstalled: false,
  bridgeRunning: false,
  claudeRunning: false,
  bridgeHealthy: false,
  runtimeReady: false,
  sourceBinaryPresent: false,
  bridgeVenvPresent: false,
  storageWarning: true,
  storageBlocked: false,
  restartBlocked: false,
  warnings: [],
};

const stateText: Record<SystemState, { title: string; detail: string }> = {
  loading: { title: "正在检查环境", detail: "读取 WSL 和服务状态…" },
  notInstalled: { title: "环境尚未就绪", detail: "需要用体检 Skill 安装或修复 WSL2 / Claude Science 运行环境" },
  stopped: { title: "Claude Science 已停止", detail: "环境完整，可以安全启动" },
  degraded: { title: "服务需要修复", detail: "部分组件正在运行，请查看诊断信息" },
  running: { title: "Claude Science 已准备好", detail: "Bridge 与应用均正常运行" },
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

const previewProviderEntryLabel = (entries: ApiKeyEntry[], provider: Provider, requestedName: string) => {
  const requested = requestedName.trim();
  if (requested) return requested;
  const date = new Date().toLocaleDateString("sv-SE");
  const prefix = `${provider.name} ${date} #`;
  const next = entries.reduce((highest, entry) => {
    if (entry.providerId !== provider.id || !entry.label.startsWith(prefix)) return highest;
    const sequence = Number.parseInt(entry.label.slice(prefix.length), 10);
    return Number.isFinite(sequence) ? Math.max(highest, sequence) : highest;
  }, 0) + 1;
  return `${prefix}${String(next).padStart(2, "0")}`;
};

const initialHealthCollapsed = () => {
  try {
    const stored = window.localStorage.getItem("csa-health-collapsed");
    return stored === null ? true : stored === "1";
  } catch {
    return true;
  }
};

const rememberHealthCollapsed = (value: boolean) => {
  try {
    window.localStorage.setItem("csa-health-collapsed", value ? "1" : "0");
  } catch {
    // The launcher remains usable when WebView storage is disabled or unavailable.
  }
};

const initialFeatureModule = (): FeatureModule => {
  try {
    const stored = window.localStorage.getItem("csa-feature-module");
    if (stored === "api-key" || stored === "subagent" || stored === "connect" || stored === "research-os") return stored;
  } catch {
    // Use the primary workflow when WebView storage is unavailable.
  }
  return "subagent";
};

const rememberFeatureModule = (value: FeatureModule) => {
  try {
    window.localStorage.setItem("csa-feature-module", value);
  } catch {
    // Module switching does not depend on persistent WebView storage.
  }
};

function App() {
  const [status, setStatus] = useState<SystemStatus>(initialStatus);
  const [providerGroups, setProviderGroups] = useState<ProviderGroup[]>(fallbackProviderGroups);
  const [activeProvider, setActiveProvider] = useState(fallbackSettings.selectedProviderId);
  const [customBaseUrl, setCustomBaseUrl] = useState(fallbackSettings.customBaseUrl);
  const [customConfirmed, setCustomConfirmed] = useState(fallbackSettings.customConfirmed);
  const [activeApiKeyId, setActiveApiKeyId] = useState<string | undefined>();
  const [apiKeys, setApiKeys] = useState<ApiKeyEntry[]>(fallbackSettings.apiKeys);
  const [showKeyPicker, setShowKeyPicker] = useState(false);
  const [draftProviderId, setDraftProviderId] = useState(fallbackSettings.selectedProviderId);
  const [draftApiKey, setDraftApiKey] = useState("");
  const [draftDisplayName, setDraftDisplayName] = useState("");
  const [draftBaseUrl, setDraftBaseUrl] = useState("");
  const [draftModel, setDraftModel] = useState("");
  const [draftModelAliases, setDraftModelAliases] = useState<ModelAlias[]>([]);
  const [draftConfirmed, setDraftConfirmed] = useState(false);
  const [testPrompt, setTestPrompt] = useState("Reply only: OK");
  const [testResult, setTestResult] = useState<ApiKeyTestResult | undefined>();
  const [autoMapResult, setAutoMapResult] = useState<ApiKeyAutoMapResult | undefined>();
  const [testingKey, setTestingKey] = useState(false);
  const [autoMappingKey, setAutoMappingKey] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [healthCollapsed, setHealthCollapsed] = useState(initialHealthCollapsed);
  const [activeModule, setActiveModule] = useState<FeatureModule>(initialFeatureModule);
  const [showMigrationAssistant, setShowMigrationAssistant] = useState(false);
  const [migrationCopyState, setMigrationCopyState] = useState("");
  const [subagentRequests, setSubagentRequests] = useState<SubagentInboxItem[]>([]);
  const [selectedSubagentId, setSelectedSubagentId] = useState<string | undefined>();
  const [subagentHistory, setSubagentHistory] = useState<SubagentRunHistoryItem[]>([]);
  const [subagentBusy, setSubagentBusy] = useState(false);
  const [subagentMessage, setSubagentMessage] = useState("");
  const [sessionHistory, setSessionHistory] = useState<ClaudeSessionHistory | undefined>();
  const [sessionHistoryLimit, setSessionHistoryLimit] = useState(50);
  const [sessionHistoryError, setSessionHistoryError] = useState("");
  const [sessionHistoryBusy, setSessionHistoryBusy] = useState(false);
  const [researchOs, setResearchOs] = useState<ResearchOsState>({ repositories: [] });
  const [skillRepositorySource, setSkillRepositorySource] = useState("");
  const [researchBusyId, setResearchBusyId] = useState("");
  const [researchMessage, setResearchMessage] = useState("");
  const [connectState, setConnectState] = useState<ConnectRuntimeState>(initialConnectState);
  const [connectEditor, setConnectEditor] = useState<"feishu" | "telegram" | "">("");
  const [feishuAppId, setFeishuAppId] = useState("");
  const [feishuAppSecret, setFeishuAppSecret] = useState("");
  const [telegramBotToken, setTelegramBotToken] = useState("");
  const [connectPairing, setConnectPairing] = useState<ConnectPairingCode | undefined>();
  const [connectPairingQr, setConnectPairingQr] = useState("");
  const [connectRoutes, setConnectRoutes] = useState<ConnectRoute[]>([]);
  const [connectHistory, setConnectHistory] = useState<ConnectHistoryMessage[]>([]);
  const [connectWorkspacePath, setConnectWorkspacePath] = useState("");
  const [connectDraftMessage, setConnectDraftMessage] = useState("");
  const [connectorSetup, setConnectorSetup] = useState<ConnectorSetup | undefined>();
  const [browserExtensionPairing, setBrowserExtensionPairing] = useState<ConnectPairingCode | undefined>();
  const [browserExtensionInstall, setBrowserExtensionInstall] = useState<BrowserExtensionInstallInfo | undefined>();
  const [connectBusyId, setConnectBusyId] = useState("");
  const [connectMessage, setConnectMessage] = useState("");
  const refreshInFlight = useRef(false);
  const sessionHistoryInFlight = useRef(false);
  const connectDispatchInFlight = useRef(false);
  const busyRef = useRef(false);

  const isTauri = "__TAURI_INTERNALS__" in window;
  const providers = useMemo(() => providerList(providerGroups), [providerGroups]);
  const activeKeyEntry = apiKeys.find((entry) => entry.id === activeApiKeyId);
  const activeKeyProvider = providers.find((provider) => provider.id === (activeKeyEntry?.providerId || activeProvider)) || providers[0];
  const draftProvider = providers.find((provider) => provider.id === draftProviderId) || activeKeyProvider;
  const draftNeedsBaseUrl = draftProvider?.id === "custom";
  const draftIsThirdParty = draftProvider?.trust.startsWith("untrusted") || false;
  const summary = isTauri
    ? stateText[status.state]
    : { title: "浏览器预览模式", detail: "本页只预览界面，不读取本机 WSL 或服务状态" };
  const migrationRecommendation = useMemo(() => storageRecommendation(status), [status]);
  const migrationPrompt = useMemo(() => buildStorageMigrationPrompt(status), [status]);
  const selectedSubagent = useMemo(
    () => subagentRequests.find((item) => item.requestId === selectedSubagentId) || subagentRequests[0],
    [subagentRequests, selectedSubagentId],
  );
  const latestSubagentHistory = subagentHistory.length > 0 ? subagentHistory[subagentHistory.length - 1] : undefined;
  const latestSubagentSessionId = latestSubagentHistory?.agent.sessionId;
  const researchSkills = useMemo(
    () => researchOs.repositories
      .flatMap((repository) => repository.skills)
      .sort((left, right) => right.modifiedMs - left.modifiedMs || left.name.localeCompare(right.name)),
    [researchOs.repositories],
  );

  const refresh = useCallback(async () => {
    if (refreshInFlight.current || busyRef.current) return;
    refreshInFlight.current = true;
    try {
      if (!isTauri) {
        setStatus(browserPreviewStatus);
        return;
      }
      const next = await invoke<SystemStatus>("get_system_status");
      setStatus(next);
      setError("");
    } catch (reason) {
      setStatus((current) => ({ ...current, state: "error" }));
      setError(String(reason));
    } finally {
      refreshInFlight.current = false;
    }
  }, [isTauri]);

  const refreshSubagentInbox = useCallback(async () => {
    try {
      if (!isTauri) {
        const demo: SubagentInboxItem = {
          requestId: "browser-preview-demo",
          fileName: "browser-preview-demo.json",
          filePath: "reports/csa-agent-inbox/browser-preview-demo.json",
          modifiedMs: Date.now(),
          request: {
            schemaVersion: 1,
            source: "browser-preview",
            taskKind: "dataset",
            title: "Demo: 数据集下载诊断",
            cwd: "C:\\project",
            note: "浏览器预览模式：真实收件箱请打开 Tauri 启动器。",
            requestedAction: "diagnose",
            approvalMode: "manual",
            policyId: "manual-only",
            createdAt: new Date().toISOString(),
          },
        };
        setSubagentRequests([demo]);
        setSelectedSubagentId((current) => current || demo.requestId);
        return;
      }
      const items = await invoke<SubagentInboxItem[]>("list_subagent_requests");
      setSubagentRequests(items);
      setSelectedSubagentId((current) => {
        if (current && items.some((item) => item.requestId === current)) return current;
        return items[0]?.requestId;
      });
      setSubagentMessage(items.length === 0 ? "收件箱为空。沙盒写入 request.json 后点击刷新。" : "");
    } catch (reason) {
      setSubagentMessage(String(reason));
    }
  }, [isTauri]);

  const refreshSubagentHistory = useCallback(async (requestId?: string) => {
    if (!requestId) {
      setSubagentHistory([]);
      return;
    }
    try {
      if (!isTauri) {
        setSubagentHistory([]);
        return;
      }
      const items = await invoke<SubagentRunHistoryItem[]>("list_subagent_run_history", { requestId });
      setSubagentHistory(items);
    } catch (reason) {
      setSubagentHistory([]);
      setSubagentMessage(String(reason));
    }
  }, [isTauri]);

  const refreshClaudeSessionHistory = useCallback(async (
    sessionId: string,
    limit: number,
    showBusy: boolean,
  ) => {
    if (sessionHistoryInFlight.current) return;
    sessionHistoryInFlight.current = true;
    if (showBusy) setSessionHistoryBusy(true);
    try {
      if (!isTauri) {
        setSessionHistory(undefined);
        setSessionHistoryError("浏览器预览模式不读取本地 Claude Code 聊天记录。");
        return;
      }
      const history = await invoke<ClaudeSessionHistory>("read_claude_session_history", {
        sessionId,
        offset: 0,
        limit,
      });
      setSessionHistory(history);
      setSessionHistoryError("");
    } catch (reason) {
      setSessionHistory(undefined);
      setSessionHistoryError(String(reason));
    } finally {
      sessionHistoryInFlight.current = false;
      if (showBusy) setSessionHistoryBusy(false);
    }
  }, [isTauri]);

  const refreshResearchOs = useCallback(async () => {
    if (!isTauri) {
      setResearchOs({ repositories: [] });
      return;
    }
    try {
      setResearchOs(await invoke<ResearchOsState>("list_skill_repositories"));
      setResearchMessage("");
    } catch (reason) {
      setResearchMessage(String(reason));
    }
  }, [isTauri]);

  const refreshConnectState = useCallback(async () => {
    if (!isTauri) {
      setConnectState(initialConnectState);
      return;
    }
    try {
      const next = await invoke<ConnectRuntimeState>("get_connect_runtime_state");
      setConnectState(next);
      setConnectWorkspacePath((current) => current || next.defaultWorkspacePath);
      if (next.installed) {
        const [routes, history] = await Promise.all([
          invoke<ConnectRoute[]>("list_connect_routes"),
          invoke<ConnectHistoryMessage[]>("list_connect_history", { offset: 0, limit: 30 }),
        ]);
        setConnectRoutes(routes);
        setConnectHistory(history);
      } else {
        setConnectRoutes([]);
        setConnectHistory([]);
      }
      if (next.error) setConnectMessage(next.error);
    } catch (reason) {
      setConnectMessage(String(reason));
    }
  }, [isTauri]);

  const dispatchRemoteConnectMessage = useCallback(async () => {
    if (!isTauri || connectDispatchInFlight.current) return;
    connectDispatchInFlight.current = true;
    try {
      const result = await invoke<ConnectDispatchResult>("dispatch_connect_pending");
      if (result.dispatched) {
        setConnectMessage(`${result.channel === "feishu" ? "飞书" : "Telegram"}消息已投递到当前 Claude Science 会话。`);
        await refreshConnectState();
      }
    } catch (reason) {
      const text = String(reason);
      if (!text.includes("可靠队列")) setConnectMessage(text);
    } finally {
      connectDispatchInFlight.current = false;
    }
  }, [isTauri, refreshConnectState]);

  useEffect(() => {
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
    loadProviderState();
    refresh();
    refreshSubagentInbox();
    refreshResearchOs();
    refreshConnectState();
    const timer = window.setInterval(refresh, 30_000);
    const connectTimer = window.setInterval(refreshConnectState, 10_000);
    const connectDispatchTimer = window.setInterval(dispatchRemoteConnectMessage, 2_000);
    return () => {
      window.clearInterval(timer);
      window.clearInterval(connectTimer);
      window.clearInterval(connectDispatchTimer);
    };
  }, [refresh, refreshSubagentInbox, refreshResearchOs, refreshConnectState, dispatchRemoteConnectMessage, isTauri]);

  useEffect(() => {
    refreshSubagentHistory(selectedSubagent?.requestId);
  }, [refreshSubagentHistory, selectedSubagent?.requestId]);

  useEffect(() => {
    setSessionHistoryLimit(50);
    setSessionHistory(undefined);
    setSessionHistoryError("");
  }, [latestSubagentSessionId]);

  useEffect(() => {
    if (activeModule !== "subagent" || !latestSubagentSessionId) return;
    refreshClaudeSessionHistory(latestSubagentSessionId, sessionHistoryLimit, true);
    const timer = window.setInterval(() => {
      refreshClaudeSessionHistory(latestSubagentSessionId, sessionHistoryLimit, false);
    }, 2_000);
    return () => window.clearInterval(timer);
  }, [activeModule, latestSubagentSessionId, refreshClaudeSessionHistory, sessionHistoryLimit]);

  const primaryLabel = useMemo(() => {
    if (status.state === "running") return "打开 Claude Science";
    if (status.restartBlocked) return "先处理诊断问题";
    if (status.state === "notInstalled") return "安装运行环境";
    if (status.state === "degraded") return "修复并重启";
    return "启动 Claude Science";
  }, [status.state, status.restartBlocked]);

  function updateBusy(value: boolean) {
    busyRef.current = value;
    setBusy(value);
  }

  function applyLauncherState(settings: LauncherSettings) {
    setActiveProvider(settings.selectedProviderId);
    setCustomBaseUrl(settings.customBaseUrl);
    setCustomConfirmed(settings.customConfirmed);
    setActiveApiKeyId(settings.activeApiKeyId);
    setApiKeys(settings.apiKeys || []);
  }

  function primeDraft(groups = providerGroups, providerId = activeProvider, baseUrl = customBaseUrl, confirmed = customConfirmed) {
    const provider = providerList(groups).find((item) => item.id === providerId) || providerList(groups)[0];
    setDraftProviderId(provider?.id || providerId);
    setDraftApiKey("");
    setDraftDisplayName("");
    setDraftBaseUrl(provider?.id === "custom" ? baseUrl : provider?.baseUrl || "");
    setDraftModel("");
    setDraftModelAliases([]);
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
    setDraftConfirmed(false);
    setTestResult(undefined);
    setAutoMapResult(undefined);
  }

  async function runAction(command: "start_services" | "stop_services" | "restart_services" | "stop_legacy_windows_bridge") {
    updateBusy(true);
    setError("");
    try {
      setStatus(await invoke<SystemStatus>(command));
    } catch (reason) {
      setError(String(reason));
    } finally {
      updateBusy(false);
    }
  }

  async function applyDraftKey() {
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

    if (!isTauri) {
      const id = `preview-${Date.now()}`;
      const entry: ApiKeyEntry = {
        id,
        providerId: draftProvider.id,
        label: draftIsThirdParty
          ? previewProviderEntryLabel(apiKeys, draftProvider, draftDisplayName)
          : draftProvider.name,
        baseUrl: draftBaseUrl,
        model: draftModel,
        customConfirmed: draftConfirmed,
        modelAliases: draftModelAliases,
        hasSecret: Boolean(draftApiKey.trim()),
        active: true,
      };
      setApiKeys((current) => [...current.map((item) => ({ ...item, active: false })), entry]);
      setActiveApiKeyId(id);
      setActiveProvider(draftProvider.id);
      setCustomBaseUrl(draftBaseUrl);
      setCustomConfirmed(draftConfirmed);
      setShowKeyPicker(false);
      setDraftApiKey("");
      setDraftDisplayName("");
      return;
    }

    updateBusy(true);
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
      applyLauncherState(saved);
      setDraftApiKey("");
      setDraftDisplayName("");
      setDraftModelAliases([]);
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
      if (!draftModel) {
        setDraftModel(preview.selectedModel);
        setDraftModelAliases([]);
        setAutoMapResult(undefined);
      }
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
        setDraftModel(result.selectedModel);
        setDraftModelAliases([]);
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
      setError("中转服务需要先确认域名后再自动映射，避免 API Key 发到错误地址。");
      return;
    }
    if (!draftApiKey.trim()) {
      setError("请先填写 API Key，再自动映射模型。");
      return;
    }

    if (!isTauri) {
      const primaryModel = draftModel || "preview-pro-model";
      const fastModel = primaryModel.includes("fast") ? primaryModel : "preview-fast-model";
      const aliases: ModelAlias[] = [
        { id: "byok-model-0001", displayName: `BYOK 主力模型 -> ${primaryModel}`, model: primaryModel },
        { id: "claude-sonnet-5", displayName: `Claude Sonnet 5 -> ${primaryModel}`, model: primaryModel },
        { id: "claude-sonnet-4-5", displayName: `Claude Sonnet 4.5 -> ${primaryModel}`, model: primaryModel },
        { id: "claude-opus-4-8", displayName: `Claude Opus 4.8 -> ${primaryModel}`, model: primaryModel },
        { id: "claude-haiku-4-5-20251001", displayName: `Claude Haiku 4.5 / Fast -> ${fastModel}`, model: fastModel },
      ];
      setDraftModel(primaryModel);
      setDraftModelAliases(aliases);
      setAutoMapResult({
        ok: true,
        providerId: draftProvider.id,
        baseUrl: draftBaseUrl,
        upstreamMode: draftProvider.protocol.includes("anthropic") ? "anthropic" : "openai",
        primaryModel,
        fastModel,
        aliases,
        models: [primaryModel, fastModel],
        message: "浏览器预览模式：已生成自动映射示例；真实模型列表请打开 Tauri 启动器。",
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
      setDraftModel(result.primaryModel);
      setDraftModelAliases(result.aliases || []);
    } catch (reason) {
      setError(String(reason));
      setDraftModelAliases([]);
    } finally {
      setAutoMappingKey(false);
    }
  }

  async function activateKey(apiKeyId: string) {
    if (apiKeyId === activeApiKeyId) return;
    if (status.restartBlocked) {
      setError("当前诊断不允许切换 API Key；请先处理磁盘、WSL 或安装包问题。");
      return;
    }
    if (!isTauri) {
      const entry = apiKeys.find((item) => item.id === apiKeyId);
      if (!entry) return;
      setActiveApiKeyId(apiKeyId);
      setActiveProvider(entry.providerId);
      setApiKeys((current) => current.map((item) => ({ ...item, active: item.id === apiKeyId })));
      return;
    }
    updateBusy(true);
    setError("");
    try {
      applyLauncherState(await invoke<LauncherSettings>("activate_api_key", { apiKeyId }));
    } catch (reason) {
      setError(String(reason));
    } finally {
      updateBusy(false);
    }
  }

  async function deleteKey(apiKeyId: string) {
    if (!isTauri) {
      setApiKeys((current) => current.filter((item) => item.id !== apiKeyId));
      return;
    }
    updateBusy(true);
    setError("");
    try {
      applyLauncherState(await invoke<LauncherSettings>("delete_api_key", { apiKeyId }));
    } catch (reason) {
      setError(String(reason));
    } finally {
      updateBusy(false);
    }
  }

  async function primaryAction() {
    if (status.state === "running") {
      try {
        await openUrl(await invoke<string>("get_claude_url"));
      } catch (reason) {
        setError(String(reason));
      }
      return;
    }
    if (status.restartBlocked) {
      const location = status.wslStoragePath || "当前 WSL 虚拟磁盘";
      setError(`当前不适合自动启动或重启（${location}）。请先根据诊断信息检查磁盘空间、WSL 状态或重新解压完整安装包；启动器不会冒险修改环境。`);
      return;
    }
    if (status.state === "degraded") return runAction("restart_services");
    if (status.state === "notInstalled") {
      setError("请先在解压目录运行体检 Skill：repair-approved.ps1 -PlanOnly；确认计划后再执行 -ApproveInstall -StartServices。");
      return;
    }
    return runAction("start_services");
  }

  async function openDashboard() {
    try {
      await openUrl(await invoke<string>("get_dashboard_url"));
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

  async function createDemoSubagentRequest() {
    setSubagentBusy(true);
    setSubagentMessage("");
    try {
      if (!isTauri) {
        await refreshSubagentInbox();
        setSubagentMessage("浏览器预览模式已生成内存 demo。");
        return;
      }
      const item = await invoke<SubagentInboxItem>("create_demo_subagent_request");
      await refreshSubagentInbox();
      setSelectedSubagentId(item.requestId);
      setSubagentMessage(`已写入 ${item.fileName}`);
    } catch (reason) {
      setSubagentMessage(String(reason));
    } finally {
      setSubagentBusy(false);
    }
  }

  async function runSelectedSubagent() {
    if (!selectedSubagent) {
      setSubagentMessage("请先选择一个 Subagent request。");
      return;
    }
    if (selectedSubagent.parseError) {
      setSubagentMessage("当前 request JSON 无效，不能运行。");
      return;
    }
    setSubagentBusy(true);
    setSubagentMessage("");
    try {
      if (!isTauri) {
        const previewRun: SubagentRunResult = {
          runId: "browser-preview-run",
          requestId: selectedSubagent.requestId,
          resultDir: "reports/csa-agent-runs/browser-preview-run",
          resultJsonPath: "reports/csa-agent-runs/browser-preview-run/result.json",
          agent: {
            ok: true,
            tool: "browser-preview",
            durationMs: 0,
            stdout: "浏览器预览模式不会启动本机 Claude Code。",
            stderr: "",
            resultText: "Preview session is ready.",
            sessionId: "preview-session",
            resumeCommand: "claude --resume preview-session -p \"<message>\"",
            message: "Subagent Hub 预览运行完成。",
          },
        };
        setSubagentHistory([{
          runId: previewRun.runId,
          kind: "run",
          requestId: previewRun.requestId,
          resultDir: previewRun.resultDir,
          resultJsonPath: previewRun.resultJsonPath,
          modifiedMs: Date.now(),
          agent: previewRun.agent,
        }]);
        return;
      }
      const result = await invoke<SubagentRunResult>("run_subagent_request", {
        requestId: selectedSubagent.requestId,
        promptOverride: "",
      });
      await refreshSubagentHistory(selectedSubagent.requestId);
      setSubagentMessage(`结果已写入 ${result.resultJsonPath}`);
    } catch (reason) {
      setSubagentMessage(String(reason));
    } finally {
      setSubagentBusy(false);
    }
  }

  async function launchLatestSubagentSession() {
    if (!latestSubagentSessionId) {
      setSubagentMessage("当前任务还没有可恢复的 Claude Code session。请先批准运行一次。");
      return;
    }
    setSubagentBusy(true);
    setSubagentMessage("");
    try {
      if (!isTauri) {
        setSubagentMessage(`浏览器预览模式不会打开终端。实际命令：claude --resume ${latestSubagentSessionId}`);
        return;
      }
      const result = await invoke<ExternalSessionLaunchResult>("launch_external_claude_session", {
        sessionId: latestSubagentSessionId,
      });
      setSubagentMessage(`${result.message} 工作目录：${result.cwd}`);
    } catch (reason) {
      setSubagentMessage(String(reason));
    } finally {
      setSubagentBusy(false);
    }
  }

  async function addSkillRepository() {
    const source = skillRepositorySource.trim();
    if (!source) {
      setResearchMessage("请先填写 HTTPS、SSH 或本机绝对路径 Git 仓库。");
      return;
    }
    if (!isTauri) {
      setResearchMessage("浏览器预览不克隆仓库，请在 Tauri 客户端中添加。");
      return;
    }
    setResearchBusyId("add");
    setResearchMessage("");
    try {
      const state = await invoke<ResearchOsState>("add_skill_repository", { source });
      setResearchOs(state);
      setSkillRepositorySource("");
      setResearchMessage(`仓库已添加，发现 ${state.repositories[0]?.skills.length || 0} 个 Skill。`);
    } catch (reason) {
      setResearchMessage(String(reason));
    } finally {
      setResearchBusyId("");
    }
  }

  async function syncSkillRepository(repositoryId: string) {
    if (!isTauri) return;
    setResearchBusyId(repositoryId);
    setResearchMessage("");
    try {
      const state = await invoke<ResearchOsState>("sync_skill_repository", { repositoryId });
      setResearchOs(state);
      const repository = state.repositories.find((item) => item.id === repositoryId);
      const newCount = repository?.skills.filter((item) => item.isNew).length || 0;
      setResearchMessage(newCount > 0 ? `同步完成，发现 ${newCount} 个新增或更新 Skill。` : "同步完成，没有新的 Skill。" );
    } catch (reason) {
      setResearchMessage(String(reason));
    } finally {
      setResearchBusyId("");
    }
  }

  async function saveFeishuBot() {
    if (!feishuAppId.trim() || !feishuAppSecret.trim()) {
      setConnectMessage("请填写飞书企业自建应用的 App ID 和 App Secret。");
      return;
    }
    if (!isTauri) {
      setConnectMessage("浏览器预览不保存凭据，请在 Tauri 客户端中配置。");
      return;
    }
    setConnectBusyId("feishu-save");
    setConnectMessage("");
    try {
      const next = await invoke<ConnectRuntimeState>("save_feishu_bot", {
        appId: feishuAppId.trim(),
        appSecret: feishuAppSecret.trim(),
      });
      setConnectState(next);
      setFeishuAppId("");
      setFeishuAppSecret("");
      setConnectEditor("");
      setConnectMessage("飞书长连接已配置。下一步生成配对码并在机器人私聊中发送。");
      await refreshConnectState();
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function registerFeishuBot() {
    if (!isTauri) {
      setConnectMessage("飞书扫码创建需要在 CSA 桌面客户端中使用。");
      return;
    }
    setConnectBusyId("feishu-register");
    setConnectMessage("正在向飞书申请一次性创建链接…");
    try {
      const started = await invoke<FeishuRegistrationStart>("start_feishu_registration");
      await openUrl(started.verificationUrl);
      setConnectMessage("请在打开的飞书官方页面登录或扫码确认，CSA 会自动完成配置。");
      const intervalMs = Math.max(2, started.intervalSeconds) * 1000;
      while (Date.now() < started.expiresAt) {
        await new Promise((resolve) => window.setTimeout(resolve, intervalMs));
        const result = await invoke<FeishuRegistrationPoll>("poll_feishu_registration", {
          deviceCode: started.deviceCode,
        });
        if (result.status === "pending") continue;
        if (result.status === "completed" && result.runtime) {
          setConnectState(result.runtime);
          setConnectEditor("");
          setConnectMessage("飞书机器人已创建并启动。现在去飞书私聊机器人完成首次身份确认。");
          await refreshConnectState();
          return;
        }
        throw new Error(result.detail || "飞书扫码创建未完成");
      }
      throw new Error("飞书扫码链接已过期，请重新点击创建");
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function saveTelegramBot() {
    if (!telegramBotToken.trim()) {
      setConnectMessage("请填写 Telegram Bot Token。");
      return;
    }
    if (!isTauri) {
      setConnectMessage("浏览器预览不保存凭据，请在 Tauri 客户端中配置。");
      return;
    }
    setConnectBusyId("telegram-save");
    setConnectMessage("");
    try {
      const next = await invoke<ConnectRuntimeState>("save_telegram_bot", { botToken: telegramBotToken.trim() });
      setConnectState(next);
      setTelegramBotToken("");
      setConnectEditor("");
      setConnectMessage("Telegram 长轮询已配置。下一步生成配对码并在机器人私聊中发送。");
      await refreshConnectState();
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function clearConnectBot(channel: "feishu" | "telegram") {
    if (!isTauri) return;
    setConnectBusyId(`${channel}-clear`);
    setConnectMessage("");
    try {
      setConnectState(await invoke<ConnectRuntimeState>("clear_connect_bot", { channel }));
      setConnectPairing((current) => current?.channel === channel ? undefined : current);
      setConnectPairingQr("");
      setConnectMessage(`${channel === "feishu" ? "飞书" : "Telegram"} 双向通道已断开，加密凭据已清除。`);
      await refreshConnectState();
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function toggleConnectGateway() {
    if (!isTauri) return;
    setConnectBusyId("gateway");
    setConnectMessage("");
    try {
      const command = connectState.running ? "stop_connect_gateway" : "start_connect_gateway";
      setConnectState(await invoke<ConnectRuntimeState>(command));
      setConnectMessage(connectState.running ? "Connect Gateway 已停止。" : "Connect Gateway 已启动并在托盘后台运行。");
      await refreshConnectState();
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function generateConnectPairing(channel: "feishu" | "telegram") {
    if (!isTauri) return;
    setConnectBusyId(`${channel}-pair`);
    setConnectMessage("");
    try {
      const pairing = await invoke<ConnectPairingCode>("generate_connect_pairing_code", { channel });
      setConnectPairing(pairing);
      setConnectPairingQr(pairing.launchUrl
        ? await QRCode.toDataURL(pairing.launchUrl, {
            width: 152,
            margin: 1,
            errorCorrectionLevel: "M",
            color: { dark: "#173b29", light: "#ffffff" },
          })
        : "");
      if (channel === "telegram" && pairing.launchUrl) {
        try {
          await openUrl(pairing.launchUrl);
          setConnectMessage("Telegram 已打开。点击 Start 即可完成配对。");
        } catch {
          setConnectMessage("未能自动打开 Telegram，请使用下方配对链接。");
        }
      } else {
        setConnectMessage(`请在${channel === "feishu" ? "飞书" : " Telegram"}机器人私聊中发送 /pair ${pairing.code}`);
      }
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function revealBrowserExtensionInstall() {
    if (!isTauri) {
      setConnectMessage("浏览器预览不提供插件安装路径，请打开 CSA 桌面客户端。");
      return;
    }
    setConnectBusyId("browser-extension-install");
    setConnectMessage("");
    try {
      const info = await invoke<BrowserExtensionInstallInfo>("get_browser_extension_install_info");
      setBrowserExtensionInstall(info);
      try {
        await navigator.clipboard.writeText(info.extensionPath);
        setConnectMessage("插件目录已复制。Chrome 打开后请选择“加载已解压的扩展程序”。");
      } catch {
        setConnectMessage("请在下方复制插件目录，并在 Chrome 扩展程序页面加载。");
      }
      await openUrl(info.chromeExtensionsUrl);
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function generateBrowserExtensionPairing() {
    if (!isTauri) return;
    setConnectBusyId("browser-extension-pair");
    setConnectMessage("");
    try {
      const pairing = await invoke<ConnectPairingCode>("generate_browser_extension_pairing_code");
      setBrowserExtensionPairing(pairing);
      setConnectMessage("正在等待 CSA Connector 自动完成配对；保持 Claude Science 页面打开即可。");
      await refreshConnectState();
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function clearBrowserExtensionPairing() {
    if (!isTauri) return;
    setConnectBusyId("browser-extension-clear");
    setConnectMessage("");
    try {
      setConnectState(await invoke<ConnectRuntimeState>("clear_browser_extension_pairing"));
      setBrowserExtensionPairing(undefined);
      setConnectMessage("浏览器插件配对已断开。");
      await refreshConnectState();
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function bindConnectRoute(routeKey: string) {
    const workspacePath = connectWorkspacePath.trim();
    if (!workspacePath) {
      setConnectMessage("请先填写要绑定的 Claude Science 工作区路径。");
      return;
    }
    if (!isTauri) return;
    setConnectBusyId(`bind-${routeKey}`);
    setConnectMessage("");
    try {
      await invoke<ConnectRoute>("bind_connect_route", { routeKey, workspacePath });
      setConnectMessage("聊天线程已绑定，等待中的消息已进入 Claude Science 项目队列。");
      await refreshConnectState();
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function sendConnectLocalMessage() {
    const text = connectDraftMessage.trim();
    if (!text) {
      setConnectMessage("请输入要发送给 Claude Science 的消息。");
      return;
    }
    const workspacePath = connectWorkspacePath.trim() || connectState.defaultWorkspacePath;
    if (!isTauri) {
      const now = Date.now();
      setConnectHistory((current) => [{
        messageId: `preview-local-${now}`,
        channel: "telegram",
        platformEventId: `preview-local-${now}`,
        senderId: "csa-local-user",
        conversationId: "csa-local-console",
        threadId: "direct",
        bindingId: "preview-binding",
        workspacePath,
        kind: "text",
        text,
        attachments: [],
        replyTo: "",
        direction: "inbound",
        status: "queued",
        lastError: "",
        createdAt: now,
        updatedAt: now,
      }, ...current]);
      setConnectDraftMessage("");
      setConnectMessage("浏览器预览已记录。本地真实投递请打开 CSA 桌面客户端。");
      return;
    }
    setConnectBusyId("local-send");
    setConnectMessage("");
    try {
      const result = await invoke<ConnectLocalSendResult>("send_connect_local_message", { text, workspacePath });
      setConnectDraftMessage("");
      setConnectMessage(result.message);
      await refreshConnectState();
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function installConnectSkill() {
    if (!isTauri) return;
    setConnectBusyId("skill");
    setConnectMessage("");
    try {
      setConnectState(await invoke<ConnectRuntimeState>("install_connect_skill"));
      setConnectMessage("CSA Connect Bridge Skill 已安装到 Claude Science。");
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function revealConnectorSetup() {
    if (!isTauri) return;
    setConnectBusyId("connector");
    setConnectMessage("");
    try {
      setConnectorSetup(await invoke<ConnectorSetup>("get_connector_setup"));
      setConnectMessage("连接令牌仅用于 Claude Science 本机 Connector，请勿发送到聊天或仓库。");
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  async function copyConnectorSetup() {
    if (!connectorSetup) return;
    try {
      await navigator.clipboard.writeText(`${connectorSetup.url}\nAuthorization: ${connectorSetup.authorizationHeader}`);
      setConnectMessage("Connector 地址和 Authorization Header 已复制。");
    } catch {
      setConnectMessage("系统未允许复制，请在下方手动选择连接信息。");
    }
  }

  async function clearConnectHistory() {
    if (!isTauri) return;
    setConnectBusyId("history-clear");
    try {
      const deleted = await invoke<number>("clear_connect_history");
      setConnectMessage(`已清除 ${deleted} 条已完成或失败的 Connect 历史；待处理消息保留。`);
      await refreshConnectState();
    } catch (reason) {
      setConnectMessage(String(reason));
    } finally {
      setConnectBusyId("");
    }
  }

  const bridgeDetail = !isTauri
    ? "仅桌面端检测"
    : status.bridgeHealthy
    ? (status.bridgePid ? `PID ${status.bridgePid}` : "健康")
    : status.bridgeRunning
      ? (status.bridgePid ? `PID ${status.bridgePid}，健康检查失败` : "服务/端口存在，健康检查失败")
      : "已停止";
  const claudeDetail = !isTauri
    ? "仅桌面端检测"
    : status.claudeRunning
    ? (status.claudePid ? `PID ${status.claudePid}` : "端口已监听")
    : "已停止";
  const storageDetail = !isTauri
    ? "仅桌面端检测"
    : status.wslStoragePath
    ? `${status.wslStoragePath}${typeof status.wslStorageFreeGb === "number" ? ` · 宿主盘剩余 ${status.wslStorageFreeGb.toFixed(1)} GB` : ""}${typeof status.wslRootFreeGb === "number" ? ` · Linux 剩余 ${status.wslRootFreeGb.toFixed(1)} GB` : ""}`
    : "未定位 WSL 虚拟磁盘";

  return (
    <main className="app-shell">
      <header className="topbar compact-topbar">
        <div className="brand-mark">CSA</div>
        <div className="topbar-brand-copy">
          <h1>CSA - Claude Science Assistant</h1>
          <p>{summary.title}</p>
        </div>
        <div className="topbar-status" aria-label="系统总状态">
          {!isTauri && <span className="status-chip warn">网页预览</span>}
          <span className={`status-chip ${status.wslInstalled ? "ok" : "warn"}`}>WSL {status.wslInstalled ? "正常" : "待处理"}</span>
          <span className={`status-chip ${status.bridgeHealthy ? "ok" : "warn"}`}>Bridge {status.bridgeHealthy ? "正常" : "未就绪"}</span>
          <span className={`status-chip ${status.claudeRunning ? "ok" : "warn"}`}>Claude {status.claudeRunning ? "运行中" : "已停止"}</span>
          <span className={`status-chip ${activeKeyEntry ? "ok" : "warn"}`}>API {activeKeyEntry ? "已配置" : "未配置"}</span>
          <button className="quiet-button" onClick={refresh} disabled={busy}>刷新</button>
          <button className="topbar-primary" onClick={primaryAction} disabled={busy || status.state === "loading"}>
            {busy ? "处理中…" : primaryLabel}
          </button>
        </div>
      </header>

      <section className={`health-panel ${healthCollapsed ? "collapsed" : ""}`} aria-label="环境检查">
        <div className="health-panel-head">
          <div>
            <strong>环境状态</strong>
            <small>{!isTauri ? "演示界面 · 不读取本机状态" : healthCollapsed ? "6 项状态已收起" : "6 项状态 · 2 行 × 3 列"}</small>
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
            <HealthItem label="WSL2" ok={status.wslInstalled} detail={!isTauri ? "仅桌面端检测" : status.distro || "未检测到"} />
            <HealthItem label="运行时" ok={status.runtimeReady} detail={!isTauri ? "仅桌面端检测" : status.runtimeReady ? "已准备" : "需体检/修复"} />
            <HealthItem label="Bridge" ok={status.bridgeHealthy} detail={bridgeDetail} />
            <HealthItem label="Claude Science" ok={status.claudeRunning} detail={claudeDetail} />
            <HealthItem
              label="WSL 存储"
              ok={!status.storageWarning}
              detail={storageDetail}
              actionLabel={migrationRecommendation.actionLabel}
              onAction={openMigrationAssistant}
            />
            <HealthItem label="当前 API Key" ok={Boolean(activeKeyEntry)} detail={activeKeyEntry?.label || "未添加"} />
          </div>
        )}
      </section>

      {(error || status.warnings.length > 0) && (
        <details className="notice compact-notice" open={Boolean(error)}>
          <summary>诊断信息 · {(error ? 1 : 0) + status.warnings.length} 项</summary>
          {error && <p>{error}</p>}
          {status.warnings.map((warning) => <p key={warning}>{warning}</p>)}
          {status.windowsBridgePid && status.bridgeRunning && (
            <button className="notice-action" onClick={() => runAction("stop_legacy_windows_bridge")} disabled={busy}>
              停止旧 Windows Bridge（PID {status.windowsBridgePid}）
            </button>
          )}
          {status.storageWarning && (
            <button className="notice-action" onClick={openMigrationAssistant} disabled={busy}>
              {migrationRecommendation.actionLabel}：交给 Codex
            </button>
          )}
        </details>
      )}

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
              <div><span>发行版</span><strong>{status.distro || "未检测到"}</strong></div>
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

      <nav className="feature-tabs" aria-label="CSA 功能模块">
        {([
          ["api-key", "API Key"],
          ["subagent", "Subagent"],
          ["connect", "Connect"],
          ["research-os", "Research OS"],
        ] as Array<[FeatureModule, string]>).map(([moduleId, label]) => (
          <button
            type="button"
            key={moduleId}
            className={activeModule === moduleId ? "active" : ""}
            aria-pressed={activeModule === moduleId}
            onClick={() => {
              setActiveModule(moduleId);
              rememberFeatureModule(moduleId);
            }}
          >
            {label}
          </button>
        ))}
      </nav>

      {activeModule === "subagent" && (
      <section className="module-section subagent-module">
        <div className="section-heading">
          <div>
            <span className="eyebrow">Agent Ops</span>
            <h2>外部 Agent 调用</h2>
          </div>
          <p>当沙盒内下载、安装或虚拟机检查卡住时，把只读诊断任务交给本机 Claude Code，并在这里收集结果。</p>
        </div>

        <div className="subagent-hub">
          <div className="subagent-head">
            <div>
              <span className="eyebrow">Subagent Hub</span>
              <h3>文件收件箱</h3>
              <p>沙盒写入 `reports/csa-agent-inbox/*.json`，这里读取请求；手动批准后才启动外部 Claude Code。</p>
            </div>
            <div className="subagent-actions">
              <button type="button" onClick={refreshSubagentInbox} disabled={subagentBusy}>刷新</button>
              <button type="button" onClick={createDemoSubagentRequest} disabled={subagentBusy}>写入 Demo</button>
              <button type="button" className="primary-inline-button" onClick={runSelectedSubagent} disabled={subagentBusy || !selectedSubagent || Boolean(selectedSubagent?.parseError)}>
                {subagentBusy ? "处理中..." : "批准运行"}
              </button>
            </div>
          </div>

          <div className="subagent-layout">
            <div className="subagent-inbox">
              {subagentRequests.length === 0 && <div className="subagent-empty">暂无请求。先写入 demo，或让沙盒生成 request.json。</div>}
              {subagentRequests.map((item) => (
                <button
                  type="button"
                  key={item.requestId}
                  className={`subagent-row ${selectedSubagent?.requestId === item.requestId ? "selected" : ""} ${item.parseError ? "invalid" : ""}`}
                  onClick={() => {
                    setSelectedSubagentId(item.requestId);
                    setSubagentMessage("");
                  }}
                >
                  <span>{item.request?.taskKind || "invalid"}</span>
                  <strong>{item.request?.title || item.fileName}</strong>
                  <small>{new Date(item.modifiedMs).toLocaleString()}</small>
                </button>
              ))}
            </div>

            <div className="subagent-detail">
              {selectedSubagent ? (
                <>
                  <div className="subagent-detail-head">
                    <div>
                      <strong>{selectedSubagent.request?.title || selectedSubagent.fileName}</strong>
                      <small>{selectedSubagent.requestId}</small>
                    </div>
                    <span className={selectedSubagent.parseError ? "subagent-status fail" : "subagent-status"}>{selectedSubagent.parseError ? "JSON 错误" : selectedSubagent.request?.approvalMode || "manual"}</span>
                  </div>
                  {selectedSubagent.parseError ? (
                    <p className="subagent-error">{selectedSubagent.parseError}</p>
                  ) : (
                    <>
                      <div className="subagent-facts">
                        <span>来源 <strong>{selectedSubagent.request?.source || "sandbox"}</strong></span>
                        <span>动作 <strong>{selectedSubagent.request?.requestedAction || "diagnose"}</strong></span>
                        <span>策略 <strong>{selectedSubagent.request?.policyId || "manual-only"}</strong></span>
                      </div>
                      <p>{selectedSubagent.request?.note || "没有补充说明。"}</p>
                      <code>{selectedSubagent.request?.cwd || selectedSubagent.filePath}</code>
                    </>
                  )}
                  {subagentMessage && <p className="subagent-message">{subagentMessage}</p>}
                  {latestSubagentSessionId && (
                    <div className="subagent-session">
                      <div className="subagent-session-head">
                        <span>latest session</span>
                        <button type="button" onClick={launchLatestSubagentSession} disabled={subagentBusy}>
                          {subagentBusy ? "打开中..." : "在 Claude Code 中继续"}
                        </button>
                      </div>
                      <code>{latestSubagentSessionId}</code>
                      <small>{`claude --resume ${latestSubagentSessionId}`}</small>
                    </div>
                  )}
                  <div className="session-history">
                    <div className="session-history-head">
                      <div>
                        <strong>聊天记录</strong>
                        <small>
                          {sessionHistoryBusy
                            ? "正在读取本地 session..."
                            : sessionHistory
                              ? `最新在前 · ${sessionHistory.totalMessages} 条 · 自动同步`
                              : "CLI 负责对话，CSA 只读记录"}
                        </small>
                      </div>
                      {latestSubagentSessionId && (
                        <button
                          type="button"
                          onClick={() => refreshClaudeSessionHistory(latestSubagentSessionId, sessionHistoryLimit, true)}
                          disabled={sessionHistoryBusy}
                        >
                          刷新记录
                        </button>
                      )}
                    </div>
                    {sessionHistoryError && <p className="subagent-error">{sessionHistoryError}</p>}
                    {!latestSubagentSessionId && (
                      <div className="subagent-empty compact">当前任务还没有 session。批准运行后即可在外部 CLI 对话并同步记录。</div>
                    )}
                    {latestSubagentSessionId && !sessionHistoryError && !sessionHistoryBusy && (sessionHistory?.messages.length ?? 0) === 0 && (
                      <div className="subagent-empty compact">session 已建立，暂时没有可展示的用户或助手消息。</div>
                    )}
                    {sessionHistory?.messages.map((message) => (
                      <article className="session-message" key={message.id}>
                        <div className="session-message-head">
                          <strong>{message.role === "assistant" ? "Claude" : "你"}</strong>
                          <time>{message.createdAt ? new Date(message.createdAt).toLocaleString() : "时间未知"}</time>
                        </div>
                        <div className="session-message-content">{message.content}</div>
                      </article>
                    ))}
                    {sessionHistory?.hasMore && (
                      <button
                        type="button"
                        className="load-older-button"
                        onClick={() => setSessionHistoryLimit((current) => Math.min(current + 50, 200))}
                        disabled={sessionHistoryLimit >= 200 || sessionHistoryBusy}
                      >
                        {sessionHistoryLimit >= 200 ? "已达到本次读取上限" : "加载更早记录"}
                      </button>
                    )}
                  </div>

                  <details className="subagent-run-history">
                    <summary>运行记录 · {subagentHistory.length} 条</summary>
                    <div className="subagent-history">
                      {subagentHistory.length === 0 ? (
                        <div className="subagent-empty compact">暂无运行记录。</div>
                      ) : (
                        subagentHistory.map((item, index) => (
                          <div className={`subagent-history-item ${item.agent.ok ? "ok" : "fail"}`} key={item.runId}>
                            <div className="subagent-history-item-head">
                              <strong>{item.kind === "continue" ? `continue #${index + 1}` : `run #${index + 1}`}</strong>
                              <span>{new Date(item.modifiedMs).toLocaleString()}</span>
                            </div>
                            <p>{item.agent.message}</p>
                            {item.agent.resultText && <pre>{item.agent.resultText}</pre>}
                            <code>{item.resultJsonPath}</code>
                          </div>
                        ))
                      )}
                    </div>
                  </details>
                </>
              ) : (
                <div className="subagent-empty">选择一个 request 查看详情。</div>
              )}
            </div>
          </div>
        </div>

      </section>
      )}

      {activeModule === "api-key" && (
      <section className="kit-section module-section">
        <div className="section-heading">
          <div>
            <span className="eyebrow">API Key</span>
            <h2>当前接入</h2>
          </div>
          <p>先从服务商模板添加一个供应商，再启动 Claude Science。新增供应商时才展开模板，不把所有 Key 平铺在首页。</p>
        </div>

        <div className="kit-layout">
          <article className="current-kit-card">
            <div className="kit-mark">{providerInitial(activeKeyEntry ? activeKeyProvider : undefined)}</div>
            <div className="kit-main">
              <span className="eyebrow">正在使用</span>
              <h3>{activeKeyEntry ? activeKeyEntry.label : "未添加供应商"}</h3>
              <p>{activeKeyEntry ? activeKeyProvider?.meta : "请添加一个供应商后再启动服务"}</p>
              <div className="kit-meta">
                {activeKeyEntry && activeKeyProvider && <span className={`trust-badge badge-${badgeClass[activeKeyProvider.badge]}`}>{activeKeyProvider.badge}</span>}
                {activeKeyEntry?.hasSecret && <span>Key 已加密保存</span>}
                {activeKeyEntry?.model && <span>模型 {activeKeyEntry.model}</span>}
                {(activeKeyEntry?.modelAliases?.length ?? 0) > 0 && <span>映射 {activeKeyEntry?.modelAliases?.length ?? 0} 条</span>}
                {activeKeyEntry?.baseUrl && <span>{activeKeyEntry.baseUrl}</span>}
              </div>
            </div>
            <button className="secondary-button" onClick={openKeyPicker} disabled={busy}>更换 / 添加供应商</button>
          </article>

          <aside className="kit-queue">
            <div className="kit-queue-head">
              <div>
                <strong>API Key 列表</strong>
                <small>按添加顺序排列，一次只激活一条</small>
              </div>
              <button onClick={openKeyPicker} disabled={busy}>添加供应商</button>
            </div>
            {apiKeys.length === 0 && <div className="key-empty">还没有供应商，点击下方按钮添加。</div>}
            {apiKeys.map((entry, index) => {
              const provider = providers.find((item) => item.id === entry.providerId);
              const active = entry.id === activeApiKeyId;
              return (
                <div className={`kit-row ${active ? "active" : ""}`} key={entry.id}>
                  <span className="kit-index">{String(index + 1).padStart(2, "0")}</span>
                  <span className="kit-row-copy">
                    <strong>{entry.label}</strong>
                    <small>{provider?.badge || "API"} · {entry.hasSecret ? "已加密保存" : "官方登录"}</small>
                  </span>
                  <span className="key-row-actions">
                    {active
                      ? <span className="active-key-label">使用中</span>
                      : <button onClick={() => activateKey(entry.id)} disabled={busy || status.restartBlocked}>使用</button>}
                    <button onClick={() => deleteKey(entry.id)} disabled={busy || active}>删除</button>
                  </span>
                </div>
              );
            })}
            <button className="add-kit-row" onClick={openKeyPicker} disabled={busy}>
              <span>+</span>
              添加新的供应商
            </button>
          </aside>
        </div>

        {showKeyPicker && (
          <div className="kit-picker" role="dialog" aria-label="添加供应商">
            <div className="kit-picker-head">
              <div>
                <span className="eyebrow">添加供应商</span>
                <h3>从模板选择，再填入你的 Key</h3>
              </div>
              <button className="quiet-button" onClick={() => setShowKeyPicker(false)} disabled={busy}>关闭</button>
            </div>

            <div className="kit-picker-grid">
              <div className="template-list">
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
                      }}
                    />
                  </label>
                )}

                {draftIsThirdParty && (
                  <label>
                    {draftNeedsBaseUrl ? "中转名称" : "配置名称"}
                    <input
                      value={draftDisplayName}
                      maxLength={80}
                      placeholder={`可留空；自动使用“${draftProvider?.name || "中转"} + 日期 + 序号”`}
                      spellCheck={false}
                      onChange={(event) => setDraftDisplayName(event.currentTarget.value)}
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
                      }}
                    />
                  </label>
                )}

                <label>
                  默认模型
                  <input
                    value={draftModel}
                    placeholder="可留空；建议先测试连通或自动映射获取模型列表"
                    spellCheck={false}
                    onChange={(event) => {
                      setDraftModel(event.currentTarget.value);
                      setTestResult(undefined);
                      setAutoMapResult(undefined);
                      setDraftModelAliases([]);
                    }}
                  />
                </label>

                {draftProvider?.id !== "claude" && (
                  <div className="test-panel">
                    <div className="test-panel-head">
                      <div>
                        <strong>测试连通</strong>
                        <small>先在这里真实对话一次；成功后会自动填入可用模型。</small>
                      </div>
                      <div className="test-panel-actions">
                        <button onClick={testDraftApiKey} disabled={busy || testingKey || autoMappingKey}>
                        {testingKey ? "正在测试…" : "测试 API Key"}
                        </button>
                        <button onClick={autoMapDraftApiKey} disabled={busy || testingKey || autoMappingKey}>
                          {autoMappingKey ? "映射中…" : "自动映射"}
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
                        {testResult.models.length > 0 && (
                          <div className="model-chip-row">
                            {testResult.models.slice(0, 8).map((item) => (
                              <button
                                className="model-chip"
                                key={item}
                                onClick={() => {
                                  setDraftModel(item);
                                  setTestResult(undefined);
                                  setAutoMapResult(undefined);
                                  setDraftModelAliases([]);
                                }}
                              >
                                {item}
                              </button>
                            ))}
                          </div>
                        )}
                      </div>
                    )}
                    {autoMapResult && (
                      <div className="mapping-result">
                        <strong>自动映射草案</strong>
                        <p>{autoMapResult.message}</p>
                        <div className="mapping-grid">
                          {autoMapResult.aliases.map((alias) => (
                            <div className="mapping-row" key={alias.id}>
                              <span>{alias.id}</span>
                              <strong>{alias.model}</strong>
                            </div>
                          ))}
                        </div>
                        {autoMapResult.models.length > 0 && (
                          <p className="mapping-hint">
                            候选模型：{autoMapResult.models.slice(0, 6).join("、")}{autoMapResult.models.length > 6 ? "…" : ""}
                          </p>
                        )}
                      </div>
                    )}
                  </div>
                )}

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
                      }}
                    />
                    我已确认该中转服务域名，API Key 只发送到该地址。
                  </label>
                )}

                <div className="form-actions">
                  <button className="primary-inline-button" onClick={applyDraftKey} disabled={busy || testingKey || autoMappingKey || status.restartBlocked}>
                    {busy ? "正在保存…" : "保存并设为当前 Key"}
                  </button>
                  <button onClick={() => setShowKeyPicker(false)} disabled={busy || testingKey || autoMappingKey}>取消</button>
                </div>
              </div>
            </div>
          </div>
        )}
      </section>
      )}

      {activeModule === "connect" && (
        <section className="module-section connect-module">
          <div className="section-heading compact-heading">
            <div>
              <span className="eyebrow">Connect</span>
              <h2>连接网关</h2>
            </div>
            <div className="connect-heading-actions">
              <span className={`module-status ${connectState.running ? "ready" : "pending"}`}>
                {connectState.running ? "后台运行" : "已停止"}
              </span>
            </div>
          </div>

          <div className="connect-summary" aria-label="Connect 队列状态">
            <div><span>通道</span><strong>{[connectState.feishu, connectState.telegram].filter((item) => item.running).length}/2</strong></div>
            <div><span>Claude Science</span><strong>{connectState.browserExtension.pageReady ? "已连接" : "未连接"}</strong></div>
            <div><span>排队</span><strong>{connectState.counts.queued}</strong></div>
            <div><span>处理中</span><strong>{connectState.counts.claimed}</strong></div>
          </div>

          {!connectState.running && (connectState.feishu.configured || connectState.telegram.configured) && (
            <div className="connect-runtime-warning" role="status">
              <strong>凭据已保存，但消息网关没有运行</strong>
              <span>{connectState.error || "请检查 WSL 与 Connect Gateway 状态。"}</span>
            </div>
          )}

          <div className="connect-block">
            <div className="connect-block-heading">
              <div><strong>消息通道</strong><small>仅接受已配对账号的私聊文本</small></div>
            </div>
            <div className="module-list connect-channel-list">
              {([
                ["feishu", "飞书", connectState.feishu],
                ["telegram", "Telegram", connectState.telegram],
              ] as const).map(([channelId, label, channel]) => (
                <div className="module-list-row connect-channel-row" key={channelId}>
                  <div className="connect-channel-copy">
                    <strong>{label}</strong>
                    <small>{channel.lastError || (channel.configured && !channel.running ? connectState.error || "Gateway 尚未启动" : channel.detail)}</small>
                  </div>
                  <span className={`module-status ${channel.running && channel.paired ? "ready" : channel.configured ? "working" : "pending"}`}>
                    {channel.paired ? "已连接" : channel.running ? "等待配对" : channel.configured ? "配置待启动" : "未连接"}
                  </span>
                  <div className="module-row-actions">
                    <button type="button" onClick={() => {
                      if (channel.running && !channel.paired) generateConnectPairing(channelId);
                      else setConnectEditor(connectEditor === channelId ? "" : channelId);
                    }} disabled={Boolean(connectBusyId)}>
                      {connectBusyId === `${channelId}-pair`
                        ? "生成中…"
                        : !channel.configured
                          ? "连接"
                          : !channel.running
                            ? "修复"
                            : !channel.paired
                              ? "配对"
                              : "管理"}
                    </button>
                  </div>
                </div>
              ))}
            </div>

            {connectPairing && (
              <div className="pairing-strip" role="status">
                {connectPairingQr && (
                  <img className="pairing-qr" src={connectPairingQr} alt="Telegram 一键配对二维码" />
                )}
                <div>
                  <span>{connectPairing.channel === "feishu" ? "飞书" : "Telegram"} 一次性配对</span>
                  <code>{connectPairing.channel === "telegram" ? `/start ${connectPairing.code}` : `/pair ${connectPairing.code}`}</code>
                </div>
                <div className="module-row-actions">
                  {connectPairing.channel === "telegram" && connectPairing.launchUrl && (
                    <button type="button" onClick={() => openUrl(connectPairing.launchUrl!)}>打开 Telegram</button>
                  )}
                  <small>{new Date(connectPairing.expiresAt).toLocaleTimeString()} 前有效</small>
                </div>
              </div>
            )}

            {connectEditor === "feishu" && (
              <form className="connect-editor connect-bot-editor" onSubmit={(event) => {
                event.preventDefault();
                saveFeishuBot();
              }}>
                <div className="feishu-register-action">
                  <button type="button" onClick={registerFeishuBot} disabled={Boolean(connectBusyId)}>
                    {connectBusyId === "feishu-register" ? "等待飞书确认…" : "扫码创建飞书机器人"}
                  </button>
                  <small>推荐：由飞书官方页面自动创建应用、配置权限和长连接。</small>
                  {connectState.feishu.configured && (
                    <button type="button" className="danger-text-button" onClick={() => clearConnectBot("feishu")} disabled={Boolean(connectBusyId)}>断开</button>
                  )}
                </div>
                <div>
                  <label htmlFor="feishu-app-id">App ID</label>
                  <input id="feishu-app-id" value={feishuAppId} onChange={(event) => setFeishuAppId(event.target.value)} placeholder="cli_..." autoComplete="off" spellCheck={false} />
                </div>
                <div>
                  <label htmlFor="feishu-app-secret">App Secret</label>
                  <input id="feishu-app-secret" type="password" value={feishuAppSecret} onChange={(event) => setFeishuAppSecret(event.target.value)} autoComplete="new-password" spellCheck={false} />
                </div>
                <button type="submit" disabled={Boolean(connectBusyId) || !feishuAppId.trim() || !feishuAppSecret.trim()}>
                  {connectBusyId === "feishu-save" ? "保存中…" : "保存并启动"}
                </button>
                <small>已有企业自建应用时，也可以手动填写 App ID 与 App Secret。</small>
              </form>
            )}

            {connectEditor === "telegram" && (
              <form className="connect-editor connect-bot-editor telegram-bot-editor" onSubmit={(event) => {
                event.preventDefault();
                saveTelegramBot();
              }}>
                <div>
                  <label htmlFor="telegram-bot-token">Bot Token</label>
                  <input id="telegram-bot-token" type="password" value={telegramBotToken} onChange={(event) => setTelegramBotToken(event.target.value)} placeholder="123456789:ABC..." autoComplete="new-password" spellCheck={false} />
                </div>
                <div className="connect-editor-actions">
                  <button type="submit" disabled={Boolean(connectBusyId) || !telegramBotToken.trim()}>
                    {connectBusyId === "telegram-save" ? "保存中…" : "保存并启动"}
                  </button>
                  {connectState.telegram.configured && (
                    <button type="button" className="danger-text-button" onClick={() => clearConnectBot("telegram")} disabled={Boolean(connectBusyId)}>断开</button>
                  )}
                </div>
                <small>无需填写 Chat ID；配对后只允许该私聊账号进入队列。</small>
              </form>
            )}
          </div>

          <div className="connect-block">
            <div className="connect-block-heading">
              <div><strong>项目路由</strong><small>聊天线程首次发消息后在这里绑定工作区</small></div>
              {connectRoutes.some((route) => !route.bindingId) && (
                <input
                  className="workspace-path-input"
                  value={connectWorkspacePath}
                  onChange={(event) => setConnectWorkspacePath(event.target.value)}
                  placeholder="当前工作区路径"
                  spellCheck={false}
                />
              )}
            </div>
            {connectRoutes.length === 0 ? (
              <div className="connect-empty-row">配对后发送一条普通消息，这里会出现待绑定线程。</div>
            ) : (
              <div className="connect-route-list">
                {connectRoutes.map((route) => (
                  <div className="connect-route-row" key={route.routeKey}>
                    <div>
                      <strong>{route.channel === "feishu" ? "飞书" : "Telegram"} · {route.senderId.slice(-8)}</strong>
                      <small>{route.workspacePath || "尚未绑定项目"}{route.pendingMessages > 0 ? ` · ${route.pendingMessages} 条待处理` : ""}</small>
                    </div>
                    <span className={`module-status ${route.bindingId ? "ready" : "pending"}`}>{route.bindingId ? "已绑定" : "待绑定"}</span>
                    <button type="button" onClick={() => bindConnectRoute(route.routeKey)} disabled={Boolean(connectBusyId) || !connectWorkspacePath.trim()}>
                      {connectBusyId === `bind-${route.routeKey}` ? "绑定中…" : route.bindingId ? "重新绑定" : "绑定"}
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>

          <details className="connect-settings">
            <summary>
              <div><strong>连接设置</strong><small>浏览器桥接、回复连接与链路测试</small></div>
              <span>{connectState.browserExtension.pageReady && connectState.mcpReady ? "已就绪" : "需要检查"}</span>
            </summary>
            <div className="connect-settings-body">
              <div className="connect-setting-row">
                <div>
                  <strong>Claude Science 页面</strong>
                  <small>{connectState.browserExtension.lastError || connectState.browserExtension.pageTitle || "浏览器插件负责消息投递"}</small>
                </div>
                <span className={`module-status ${connectState.browserExtension.pageReady ? "ready" : connectState.browserExtension.paired ? "working" : "pending"}`}>
                  {connectState.browserExtension.pageReady ? "页面就绪" : connectState.browserExtension.paired ? "等待页面" : "未连接"}
                </span>
                {!connectState.browserExtension.paired && (
                  <button type="button" onClick={browserExtensionInstall ? generateBrowserExtensionPairing : revealBrowserExtensionInstall} disabled={Boolean(connectBusyId)}>
                    {browserExtensionInstall ? "连接插件" : "安装插件"}
                  </button>
                )}
              </div>

              {browserExtensionPairing && (
                <div className="pairing-strip" role="status">
                  <div><span>插件配对码</span><code>{browserExtensionPairing.code}</code></div>
                  <small>{new Date(browserExtensionPairing.expiresAt).toLocaleTimeString()} 前有效</small>
                </div>
              )}
              {browserExtensionInstall && !connectState.browserExtension.paired && (
                <div className="connector-secret-panel browser-extension-install-panel">
                  <div><span>插件目录</span><code>{browserExtensionInstall.extensionPath}</code></div>
                </div>
              )}

              <div className="connect-setting-row">
                <div><strong>回复连接</strong><small>Skill 与 MCP 将 Claude Science 回复送回原通道</small></div>
                <span className={`module-status ${connectState.skillInstalled && connectState.mcpReady ? "ready" : "pending"}`}>
                  {connectState.skillInstalled && connectState.mcpReady ? "已就绪" : "未就绪"}
                </span>
                <button type="button" onClick={connectState.skillInstalled ? revealConnectorSetup : installConnectSkill} disabled={Boolean(connectBusyId) || (connectState.skillInstalled && !connectState.mcpReady)}>
                  {connectState.skillInstalled ? "连接信息" : "安装 Skill"}
                </button>
              </div>

              {connectorSetup && (
                <div className="connector-secret-panel">
                  <div><span>URL</span><code>{connectorSetup.url}</code></div>
                  <div><span>Authorization</span><code>{connectorSetup.authorizationHeader}</code></div>
                  <div className="module-row-actions">
                    <button type="button" onClick={copyConnectorSetup}>复制</button>
                    <button type="button" onClick={() => setConnectorSetup(undefined)}>隐藏</button>
                  </div>
                </div>
              )}

              <form className="connect-local-composer" onSubmit={(event) => {
                event.preventDefault();
                sendConnectLocalMessage();
              }}>
                <div><strong>链路测试</strong><small>向当前 Claude Science 投递一条测试消息</small></div>
                <input value={connectDraftMessage} onChange={(event) => setConnectDraftMessage(event.currentTarget.value)} placeholder="输入测试消息" spellCheck={false} />
                <button type="submit" disabled={Boolean(connectBusyId) || !connectDraftMessage.trim()}>发送</button>
              </form>

              <details className="connect-maintenance">
                <summary>维护操作</summary>
                <div className="module-row-actions">
                  <button type="button" onClick={toggleConnectGateway} disabled={Boolean(connectBusyId) || (!connectState.feishu.configured && !connectState.telegram.configured)}>
                    {connectState.running ? "停止 Gateway" : "启动 Gateway"}
                  </button>
                  <button type="button" onClick={clearBrowserExtensionPairing} disabled={Boolean(connectBusyId) || !connectState.browserExtension.paired}>断开插件</button>
                  <button type="button" onClick={clearConnectHistory} disabled={Boolean(connectBusyId) || connectHistory.length === 0}>清理已完成记录</button>
                </div>
              </details>
            </div>
          </details>

          <div className="connect-block connect-history-block">
            <div className="connect-block-heading">
              <div><strong>最近记录</strong><small>最新消息在前，仅供审计与回看</small></div>
            </div>
            {connectHistory.length === 0 ? (
              <div className="connect-empty-row">暂无消息记录</div>
            ) : (
              <div className="connect-history-list">
                {connectHistory.map((message) => (
                  <div className="connect-history-row" key={message.messageId}>
                    <span className={`history-direction ${message.direction}`}>{message.direction === "inbound" ? "收到" : "回复"}</span>
                    <div>
                      <p>{message.text || (message.attachments.length > 0 ? "图片消息" : "")}</p>
                      {message.attachments.length > 0 && (
                        <small className="connect-history-attachment">图片 · {message.attachments.length} 张</small>
                      )}
                      <small>{message.senderId === "csa-local-user" ? "本地输入" : message.channel} · {message.status} · {new Date(message.createdAt).toLocaleString()}</small>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>

          {connectMessage && <p className="research-message connect-message" role="status">{connectMessage}</p>}
          <p className="module-footnote">消息默认保留 30 天。聊天端不能直接执行安装、下载或宿主机命令；此类请求只会标记为等待本地审批。</p>
        </section>
      )}

      {activeModule === "research-os" && (
        <section className="module-section simple-module">
          <div className="section-heading compact-heading">
            <div>
              <span className="eyebrow">Research OS</span>
              <h2>Git Skill Feed</h2>
            </div>
            <p>连接 Skill 仓库，按更新时间显示新增能力。</p>
          </div>
          <form className="repository-entry" onSubmit={(event) => {
            event.preventDefault();
            addSkillRepository();
          }}>
            <label htmlFor="skill-repository-url">Skill 仓库地址</label>
            <input
              id="skill-repository-url"
              value={skillRepositorySource}
              onChange={(event) => setSkillRepositorySource(event.target.value)}
              placeholder="https://github.com/your-org/research-skills.git"
              spellCheck={false}
              disabled={Boolean(researchBusyId)}
            />
            <button type="submit" disabled={Boolean(researchBusyId) || !skillRepositorySource.trim()}>
              {researchBusyId === "add" ? "正在克隆…" : "添加仓库"}
            </button>
          </form>
          <p className="module-footnote">使用系统 Git 浅克隆；只读取仓库中受 Git 跟踪的 SKILL.md，不执行仓库脚本。</p>
          {researchMessage && <p className="research-message" role="status">{researchMessage}</p>}

          {researchOs.repositories.length === 0 ? (
            <div className="module-empty-state">
              <strong>尚未添加 Skill 仓库</strong>
              <p>添加仓库后，这里会显示同步版本、Skill 数量和最新能力。</p>
            </div>
          ) : (
            <div className="repository-list" aria-label="Skill 仓库列表">
              {researchOs.repositories.map((repository) => (
                <div className="repository-row" key={repository.id}>
                  <div className="repository-copy">
                    <strong>{repository.source}</strong>
                    <small>{repository.skills.length} Skills · {repository.lastCommit || "版本未知"} · {new Date(repository.lastSyncedAt).toLocaleString()}</small>
                  </div>
                  <button
                    type="button"
                    onClick={() => syncSkillRepository(repository.id)}
                    disabled={Boolean(researchBusyId)}
                  >
                    {researchBusyId === repository.id ? "同步中…" : "同步"}
                  </button>
                </div>
              ))}
            </div>
          )}

          {researchSkills.length > 0 && (
            <div className="skill-feed">
              <div className="skill-feed-head">
                <strong>Skill Feed</strong>
                <span>{researchSkills.length} 项 · 最新在前</span>
              </div>
              {researchSkills.map((skill) => (
                <article className="skill-feed-row" key={skill.id}>
                  <div>
                    <div className="skill-name-line">
                      <strong>{skill.name}</strong>
                      {skill.isNew && <span>NEW</span>}
                    </div>
                    <p>{skill.description}</p>
                  </div>
                  <code>{skill.relativePath}</code>
                </article>
              ))}
            </div>
          )}
        </section>
      )}

      <footer>
        <span>{status.linuxUser && status.distro ? `${status.linuxUser} · ${status.distro}` : "Windows 10/11 · WSL2"}</span>
        <div className="footer-actions">
          <button onClick={openDashboard} disabled={busy || !status.bridgeHealthy}>配置面板</button>
          <button onClick={() => runAction("restart_services")} disabled={busy || !status.wslInstalled || status.restartBlocked}>重启</button>
          <button onClick={() => runAction("stop_services")} disabled={busy || (!status.bridgeRunning && !status.claudeRunning)}>停止</button>
        </div>
      </footer>
    </main>
  );
}

function HealthItem({ label, ok, detail, actionLabel, onAction }: {
  label: string;
  ok: boolean;
  detail: string;
  actionLabel?: string;
  onAction?: () => void;
}) {
  return (
    <div className="health-item">
      <span className={`health-check ${ok ? "ok" : ""}`}>{ok ? "✓" : "—"}</span>
      <div className="health-item-copy"><strong>{label}</strong><small>{detail}</small></div>
      {actionLabel && onAction && <button className="health-item-action" onClick={onAction}>{actionLabel}</button>}
    </div>
  );
}

export default App;
