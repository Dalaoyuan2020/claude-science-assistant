import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import "./App.css";

type SystemState = "loading" | "notInstalled" | "stopped" | "degraded" | "running" | "error";

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
  warnings: string[];
}

interface Provider {
  id: string;
  name: string;
  meta: string;
  badge: "官方" | "聚合" | "中转" | "自定义";
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

const fallbackProviderGroups: ProviderGroup[] = [
  {
    title: "官方直连",
    tier: "official",
    providers: [
      { id: "glm", name: "GLM-5.2", meta: "智谱官方 API", badge: "官方", trust: "official", protocol: "openai-compatible", baseUrl: "https://open.bigmodel.cn/api/paas/v4", defaultModel: "glm-5.2" },
      { id: "longcat", name: "LongCat", meta: "OpenAI / Anthropic 兼容", badge: "官方", trust: "official", protocol: "openai-compatible", baseUrl: "https://api.longcat.chat/openai", defaultModel: "LongCat-2.0" },
      { id: "deepseek", name: "DeepSeek", meta: "官方 API", badge: "官方", trust: "official", protocol: "anthropic-compatible", baseUrl: "https://api.deepseek.com/anthropic", defaultModel: "deepseek-v4-pro" },
      { id: "minimax", name: "MiniMax", meta: "官方 API / Anthropic 兼容", badge: "官方", trust: "official", protocol: "anthropic-compatible", baseUrl: "https://api.minimax.io/anthropic", defaultModel: "MiniMax-M3" },
      { id: "claude", name: "Claude", meta: "官方登录 / API", badge: "官方", trust: "official", protocol: "official-login-or-api" },
      { id: "openai", name: "OpenAI / GPT", meta: "官方登录 / API", badge: "官方", trust: "official", protocol: "official-login-or-api", baseUrl: "https://api.openai.com/v1", defaultModel: "gpt-5.5" },
    ],
  },
  {
    title: "聚合平台",
    tier: "aggregator",
    providers: [
      { id: "opencode-go", name: "OpenCode Go", meta: "订阅 API Key", badge: "聚合", trust: "aggregator", protocol: "openai-compatible", baseUrl: "https://opencode.ai/zen/go/v1", defaultModel: "glm-5.2" },
      { id: "openrouter", name: "OpenRouter", meta: "多模型路由", badge: "聚合", trust: "aggregator", protocol: "openai-compatible", baseUrl: "https://openrouter.ai/api/v1" },
    ],
  },
  {
    title: "第三方中转",
    tier: "custom",
    providers: [
      { id: "builtin-relay", name: "内置中转", meta: "10521052.xyz/v1", badge: "中转", trust: "untrusted-builtin", protocol: "openai-compatible", baseUrl: "https://10521052.xyz/v1" },
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
  warnings: [],
};

const browserPreviewStatus: SystemStatus = {
  state: "stopped",
  wslInstalled: true,
  distro: "Ubuntu-24.04",
  linuxUser: "preview",
  bridgeRunning: false,
  claudeRunning: false,
  bridgeHealthy: false,
  runtimeReady: true,
  sourceBinaryPresent: true,
  bridgeVenvPresent: true,
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
  const refreshInFlight = useRef(false);

  const isTauri = "__TAURI_INTERNALS__" in window;
  const providers = useMemo(() => providerList(providerGroups), [providerGroups]);
  const activeKeyEntry = apiKeys.find((entry) => entry.id === activeApiKeyId);
  const activeKeyProvider = providers.find((provider) => provider.id === (activeKeyEntry?.providerId || activeProvider)) || providers[0];
  const draftProvider = providers.find((provider) => provider.id === draftProviderId) || activeKeyProvider;
  const draftNeedsBaseUrl = draftProvider?.id === "custom";
  const draftIsThirdParty = draftProvider?.trust.startsWith("untrusted") || false;
  const summary = stateText[status.state];

  const refresh = useCallback(async () => {
    if (refreshInFlight.current) return;
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
    const timer = window.setInterval(refresh, 30_000);
    return () => window.clearInterval(timer);
  }, [refresh, isTauri]);

  const primaryLabel = useMemo(() => {
    if (status.state === "running") return "打开 Claude Science";
    if (status.state === "notInstalled") return "安装运行环境";
    if (status.state === "degraded") return "修复并重启";
    return "启动 Claude Science";
  }, [status.state]);

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
    setDraftBaseUrl(provider?.id === "custom" ? baseUrl : provider?.baseUrl || "");
    setDraftModel(provider?.defaultModel || "");
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
    setDraftBaseUrl(provider.id === "custom" ? customBaseUrl : provider.baseUrl || "");
    setDraftModel(provider.defaultModel || "");
    setDraftModelAliases([]);
    setDraftConfirmed(false);
    setTestResult(undefined);
    setAutoMapResult(undefined);
  }

  async function runAction(command: "start_services" | "stop_services" | "restart_services" | "stop_legacy_windows_bridge") {
    setBusy(true);
    setError("");
    try {
      setStatus(await invoke<SystemStatus>(command));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function applyDraftKey() {
    if (!draftProvider) return;
    if (draftNeedsBaseUrl && !draftBaseUrl.trim()) {
      setError("请先填写自定义中转 Base URL。");
      return;
    }
    if (draftIsThirdParty && !draftConfirmed) {
      setError("第三方中转需要先确认域名，避免 API Key 被误发到不熟悉的地址。");
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
        label: draftProvider.name,
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
      return;
    }

    setBusy(true);
    setError("");
    try {
      const saved = await invoke<LauncherSettings>("save_api_key", {
        selectedProviderId: draftProvider.id,
        apiKey: draftApiKey,
        customBaseUrl: draftBaseUrl,
        customConfirmed: draftConfirmed,
        model: draftModel,
        modelAliases: draftModelAliases,
      });
      applyLauncherState(saved);
      setDraftApiKey("");
      setDraftModelAliases([]);
      setAutoMapResult(undefined);
      setShowKeyPicker(false);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function testDraftApiKey() {
    if (!draftProvider || draftProvider.id === "claude") return;
    if (draftNeedsBaseUrl && !draftBaseUrl.trim()) {
      setError("请先填写自定义中转 Base URL。");
      return;
    }
    if (draftIsThirdParty && !draftConfirmed) {
      setError("第三方中转需要先确认域名后再测试，避免 API Key 发到错误地址。");
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
        selectedModel: draftModel || draftProvider.defaultModel || "preview-model",
        reply: "OK",
        models: [draftModel || draftProvider.defaultModel || "preview-model"],
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
      setError("第三方中转需要先确认域名后再自动映射，避免 API Key 发到错误地址。");
      return;
    }
    if (!draftApiKey.trim()) {
      setError("请先填写 API Key，再自动映射模型。");
      return;
    }

    if (!isTauri) {
      const primaryModel = draftModel || draftProvider.defaultModel || "preview-pro-model";
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
    if (!isTauri) {
      const entry = apiKeys.find((item) => item.id === apiKeyId);
      if (!entry) return;
      setActiveApiKeyId(apiKeyId);
      setActiveProvider(entry.providerId);
      setApiKeys((current) => current.map((item) => ({ ...item, active: item.id === apiKeyId })));
      return;
    }
    setBusy(true);
    setError("");
    try {
      applyLauncherState(await invoke<LauncherSettings>("activate_api_key", { apiKeyId }));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function deleteKey(apiKeyId: string) {
    if (!isTauri) {
      setApiKeys((current) => current.filter((item) => item.id !== apiKeyId));
      return;
    }
    setBusy(true);
    setError("");
    try {
      applyLauncherState(await invoke<LauncherSettings>("delete_api_key", { apiKeyId }));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
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

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand-mark">CSA</div>
        <div>
          <h1>CSA - Claude Science Assistant</h1>
          <p>一个当前 API Key，一个安全启动入口</p>
        </div>
        <button className="quiet-button" onClick={refresh} disabled={busy}>刷新状态</button>
      </header>

      <section className={`hero state-${status.state}`}>
        <div className="status-orb"><span /></div>
        <div className="hero-copy">
          <span className="eyebrow">系统状态</span>
          <h2>{summary.title}</h2>
          <p>{summary.detail}</p>
        </div>
        <button className="primary-button" onClick={primaryAction} disabled={busy || status.state === "loading"}>
          {busy ? "正在处理…" : primaryLabel}
        </button>
      </section>

      <section className="health-grid" aria-label="环境检查">
        <HealthItem label="WSL2" ok={status.wslInstalled} detail={status.distro || "未检测到"} />
        <HealthItem label="运行时" ok={status.runtimeReady} detail={status.runtimeReady ? "已准备" : "需体检/修复"} />
        <HealthItem label="Bridge" ok={status.bridgeHealthy} detail={status.bridgePid ? `PID ${status.bridgePid}` : "已停止"} />
        <HealthItem label="Claude Science" ok={status.claudeRunning} detail={status.claudePid ? `PID ${status.claudePid}` : "已停止"} />
        <HealthItem label="当前 API Key" ok={Boolean(activeKeyEntry)} detail={activeKeyEntry ? activeKeyProvider?.name || activeKeyEntry.label : "未添加"} />
      </section>

      {(error || status.warnings.length > 0) && (
        <section className="notice" role="alert">
          <strong>诊断信息</strong>
          {error && <p>{error}</p>}
          {status.warnings.map((warning) => <p key={warning}>{warning}</p>)}
          {status.windowsBridgePid && status.bridgeRunning && (
            <button className="notice-action" onClick={() => runAction("stop_legacy_windows_bridge")} disabled={busy}>
              停止旧 Windows Bridge（PID {status.windowsBridgePid}）
            </button>
          )}
        </section>
      )}

      <section className="kit-section">
        <div className="section-heading">
          <div>
            <span className="eyebrow">API Key</span>
            <h2>当前接入</h2>
          </div>
          <p>先从服务商模板添加一个 API Key，再启动 Claude Science。新增 Key 时才展开模板，不把所有 Key 平铺在首页。</p>
        </div>

        <div className="kit-layout">
          <article className="current-kit-card">
            <div className="kit-mark">{providerInitial(activeKeyEntry ? activeKeyProvider : undefined)}</div>
            <div className="kit-main">
              <span className="eyebrow">正在使用</span>
              <h3>{activeKeyEntry ? activeKeyEntry.label : "未添加 API Key"}</h3>
              <p>{activeKeyEntry ? activeKeyProvider?.meta : "请添加一个 API Key 后再启动服务"}</p>
              <div className="kit-meta">
                {activeKeyEntry && activeKeyProvider && <span className={`trust-badge badge-${badgeClass[activeKeyProvider.badge]}`}>{activeKeyProvider.badge}</span>}
                {activeKeyEntry?.hasSecret && <span>Key 已加密保存</span>}
                {activeKeyEntry?.model && <span>模型 {activeKeyEntry.model}</span>}
                {(activeKeyEntry?.modelAliases?.length ?? 0) > 0 && <span>映射 {activeKeyEntry?.modelAliases?.length ?? 0} 条</span>}
                {activeKeyEntry?.baseUrl && <span>{activeKeyEntry.baseUrl}</span>}
              </div>
            </div>
            <button className="secondary-button" onClick={openKeyPicker} disabled={busy}>更换 / 添加 Key</button>
          </article>

          <aside className="kit-queue">
            <div className="kit-queue-head">
              <div>
                <strong>API Key 列表</strong>
                <small>按添加顺序排列，一次只激活一条</small>
              </div>
              <button onClick={openKeyPicker} disabled={busy}>添加 API Key</button>
            </div>
            {apiKeys.length === 0 && <div className="key-empty">还没有 API Key，点击下方按钮添加。</div>}
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
                      : <button onClick={() => activateKey(entry.id)} disabled={busy}>使用</button>}
                    <button onClick={() => deleteKey(entry.id)} disabled={busy || active}>删除</button>
                  </span>
                </div>
              );
            })}
            <button className="add-kit-row" onClick={openKeyPicker} disabled={busy}>
              <span>+</span>
              添加新的 API Key
            </button>
          </aside>
        </div>

        {showKeyPicker && (
          <div className="kit-picker" role="dialog" aria-label="添加 API Key">
            <div className="kit-picker-head">
              <div>
                <span className="eyebrow">添加 API Key</span>
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
                    placeholder={draftProvider?.defaultModel || "留空则由服务商默认决定"}
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
                    我确认这是自己选择的第三方中转，API Key 只发送到该域名。
                  </label>
                )}

                <div className="form-actions">
                  <button className="primary-inline-button" onClick={applyDraftKey} disabled={busy || testingKey || autoMappingKey}>
                    {busy ? "正在保存…" : "保存并设为当前 Key"}
                  </button>
                  <button onClick={() => setShowKeyPicker(false)} disabled={busy || testingKey || autoMappingKey}>取消</button>
                </div>
              </div>
            </div>
          </div>
        )}
      </section>

      <footer>
        <span>{status.linuxUser && status.distro ? `${status.linuxUser} · ${status.distro}` : "Windows 10/11 · WSL2"}</span>
        <div className="footer-actions">
          <button onClick={openDashboard} disabled={busy || !status.bridgeRunning}>配置面板</button>
          <button onClick={() => runAction("restart_services")} disabled={busy || !status.wslInstalled}>重启</button>
          <button onClick={() => runAction("stop_services")} disabled={busy || (!status.bridgeRunning && !status.claudeRunning)}>停止</button>
        </div>
      </footer>
    </main>
  );
}

function HealthItem({ label, ok, detail }: { label: string; ok: boolean; detail: string }) {
  return (
    <div className="health-item">
      <span className={`health-check ${ok ? "ok" : ""}`}>{ok ? "✓" : "—"}</span>
      <div><strong>{label}</strong><small>{detail}</small></div>
    </div>
  );
}

export default App;
