import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Activity,
  AlertCircle,
  ArrowRight,
  Bell,
  Check,
  CheckCircle2,
  ChevronRight,
  Circle,
  Cloud,
  Copy,
  CreditCard,
  Eye,
  EyeOff,
  ExternalLink,
  FileClock,
  FolderOpen,
  Gauge,
  Info,
  KeyRound,
  Laptop,
  Link2,
  LoaderCircle,
  Minus,
  Moon,
  Network,
  RefreshCw,
  RotateCcw,
  Route,
  Settings,
  ShieldCheck,
  Square,
  Sun,
  Trash2,
  UserRound,
  Waypoints,
  X,
  Zap,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  clearLogs,
  deleteSavedKey,
  detectStatus,
  fetchModels,
  isTauri,
  openBackups,
  openCodexHome,
  openVexluneHub,
  restoreOfficial,
  switchProvider,
} from "./bridge";
import type {
  DetectionStatus,
  ErrorPayload,
  OperationResult,
  OperationState,
  ProgressEvent,
} from "./types";

type Language = "en" | "zh";
type View = "route" | "configuration" | "activity" | "settings" | "about";
type RouteChoice = "official" | "generic";
type OperationKind = "switch" | "restore";

const SWITCH_STEPS = {
  en: [
    "Detect Codex status",
    "Close Codex",
    "Back up configuration",
    "Validate API key",
    "Update Provider",
    "Connect both channels",
    "Verify configuration",
    "Start Codex",
    "Complete",
  ],
  zh: [
    "检测 Codex 状态",
    "关闭 Codex",
    "备份配置",
    "验证 API Key",
    "更新 Provider",
    "连接双通道",
    "验证配置",
    "启动 Codex",
    "完成",
  ],
} as const;

const RESTORE_STEPS = {
  en: [
    "Detect Codex status",
    "Close Codex",
    "Back up current configuration",
    "Read protected baseline",
    "Restore Provider",
    "Keep custom API key",
    "Verify restored configuration",
    "Start Codex",
    "Complete",
  ],
  zh: [
    "检测 Codex 状态",
    "关闭 Codex",
    "备份当前配置",
    "读取受保护基线",
    "恢复 Provider",
    "保留自定义 API 密钥",
    "验证恢复结果",
    "启动 Codex",
    "完成",
  ],
} as const;

const ENGLISH_ERRORS: Record<string, string> = {
  API_URL_MISSING: "Enter an API address.",
  API_URL_FORMAT: "The API address is invalid.",
  API_URL_HOST: "The API address is missing a valid host.",
  API_URL_INSECURE: "Remote API addresses must use HTTPS. Localhost may use HTTP.",
  API_URL_SCHEME: "Use an HTTPS address, or HTTP for localhost.",
  API_URL_USERINFO: "The API address must not contain user information.",
  API_URL_COMPONENTS: "The API address must not contain a query or fragment.",
  API_KEY_MISSING: "Enter an API key or keep a saved key available.",
  API_KEY_FORMAT: "The API key format is invalid.",
  API_AUTH_401: "The API key is invalid or expired.",
  API_AUTH_403: "This API key does not have access.",
  API_RATE_LIMIT_429: "The API is rate-limiting requests. Try again later.",
  API_TIMEOUT: "The connection to the API timed out.",
  API_DNS: "The API hostname could not be resolved.",
  API_TLS: "A secure connection to the API could not be established.",
  API_PROXY: "The current network proxy could not reach the API.",
  API_CONNECT: "The application could not connect to the API.",
  API_INVALID_RESPONSE: "The API returned an unexpected response.",
  API_REDIRECT_BLOCKED: "An unexpected API redirect was blocked.",
  CODEX_002: "The Codex configuration directory was not found.",
  KEY_DELETE_ACTIVE: "Restore the official route before deleting the saved key.",
};

function initialLanguage(): Language {
  try {
    const saved = window.localStorage.getItem("vexlune-hub-language");
    if (saved === "en" || saved === "zh") return saved;
  } catch {
    // Use the system language when storage is unavailable.
  }
  return navigator.language.toLowerCase().startsWith("zh") ? "zh" : "en";
}

function initialDarkMode() {
  try {
    return window.localStorage.getItem("vexlune-hub-theme") === "dark";
  } catch {
    return false;
  }
}

function initialModel() {
  try {
    return window.localStorage.getItem("vexlune-hub-model") || "gpt-5.6-sol";
  } catch {
    return "gpt-5.6-sol";
  }
}

function normalizeApiRootInput(input: string) {
  const trimmed = input.trim();
  if (!trimmed) return "";
  try {
    const url = new URL(trimmed);
    const segments = url.pathname.split("/").filter(Boolean);
    while (segments.at(-1)?.toLowerCase() === "v1") segments.pop();
    url.pathname = segments.length ? `/${segments.join("/")}` : "";
    return url.toString().replace(/\/$/, "");
  } catch {
    return trimmed.replace(/\/+$/, "").replace(/(?:\/v1)+$/i, "");
  }
}

function initialApiUrl() {
  try {
    return normalizeApiRootInput(
      window.localStorage.getItem("vexlune-hub-generic-api-root") || "",
    );
  } catch {
    return "";
  }
}

function apiUrlReady(value: string) {
  try {
    const url = new URL(normalizeApiRootInput(value));
    return Boolean(url.host && (url.protocol === "https:" || url.protocol === "http:"));
  } catch {
    return false;
  }
}

function makeProgress(labels: readonly string[]): ProgressEvent[] {
  return labels.map((step, index) => ({
    step,
    index,
    total: labels.length,
    state: "pending",
  }));
}

function normalizeError(error: unknown): ErrorPayload {
  if (typeof error === "object" && error !== null && "message" in error) {
    const value = error as Partial<ErrorPayload>;
    return {
      code: value.code ?? "SWITCH-UNKNOWN",
      message: String(value.message),
      configChanged: Boolean(value.configChanged),
      rolledBack: Boolean(value.rolledBack),
      httpStatus: value.httpStatus,
      requestElapsedMs: value.requestElapsedMs,
      codexRestored: value.codexRestored,
    };
  }
  return {
    code: "SWITCH-UNKNOWN",
    message: "操作未完成，请重新检测后重试。",
    configChanged: false,
    rolledBack: false,
  };
}

export function App() {
  const [language, setLanguage] = useState<Language>(initialLanguage);
  const [darkMode, setDarkMode] = useState(initialDarkMode);
  const [view, setView] = useState<View>("route");
  const [desiredRoute, setDesiredRoute] = useState<RouteChoice>("official");
  const [apiUrl, setApiUrl] = useState(initialApiUrl);
  const [apiKey, setApiKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [selectedModel, setSelectedModel] = useState(initialModel);
  const [availableModels, setAvailableModels] = useState<string[]>(() => [initialModel()]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelRequestElapsed, setModelRequestElapsed] = useState<number | null>(null);
  const [status, setStatus] = useState<DetectionStatus | null>(null);
  const [operation, setOperation] = useState<OperationState>("idle");
  const [operationKind, setOperationKind] = useState<OperationKind>("switch");
  const [progress, setProgress] = useState<ProgressEvent[]>(() =>
    makeProgress(SWITCH_STEPS.en),
  );
  const [result, setResult] = useState<OperationResult | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [maximized, setMaximized] = useState(false);
  const languageRef = useRef(language);
  const keyInputRef = useRef<HTMLInputElement>(null);
  const lastAutomaticModelRequest = useRef("");

  const text = useCallback(
    (zh: string, en: string) => (language === "zh" ? zh : en),
    [language],
  );
  const activeRoute: RouteChoice =
    status?.currentRoute === "generic" ? "generic" : "official";
  const accountConnected = status?.accountConnected === true;
  const hasNewKey = apiKey.trim().length > 0;
  const canSwitch =
    operation !== "running" &&
    apiUrlReady(apiUrl) &&
    selectedModel.length > 0 &&
    (hasNewKey ? apiKey.trim().length >= 8 : status?.apiKeyStored === true);
  const canRestore =
    operation !== "running" &&
    activeRoute !== "official" &&
    status?.codexDetected === true;
  const progressLabels =
    operationKind === "restore"
      ? RESTORE_STEPS[language]
      : SWITCH_STEPS[language];

  const refresh = useCallback(async () => {
    setNotice(null);
    try {
      const detected = await detectStatus();
      setStatus(detected);
      setDesiredRoute(
        detected.currentRoute === "generic" ? "generic" : "official",
      );
      if (detected.currentRoute === "generic" && detected.currentModel) {
        setSelectedModel(detected.currentModel);
        setAvailableModels((current) =>
          current.includes(detected.currentModel as string)
            ? current
            : [detected.currentModel as string, ...current],
        );
      }
    } catch (error) {
      const normalized = normalizeError(error);
      const message =
        languageRef.current === "zh"
          ? normalized.message
          : ENGLISH_ERRORS[normalized.code] ?? "Status detection failed.";
      setNotice(`${normalized.code}: ${message}`);
    }
  }, []);

  useEffect(() => {
    languageRef.current = language;
    document.documentElement.lang = language === "zh" ? "zh-CN" : "en";
  }, [language]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!isTauri) return;
    const appWindow = getCurrentWindow();
    void appWindow.isMaximized().then(setMaximized);
    const unlisten = appWindow.onResized(() => {
      void appWindow.isMaximized().then(setMaximized);
    });
    return () => {
      void unlisten.then((dispose) => dispose()).catch(() => undefined);
    };
  }, []);

  function setAppLanguage(value: Language) {
    setLanguage(value);
    try {
      window.localStorage.setItem("vexlune-hub-language", value);
    } catch {
      // The selection remains active for this session.
    }
  }

  function toggleTheme() {
    setDarkMode((current) => {
      const next = !current;
      try {
        window.localStorage.setItem("vexlune-hub-theme", next ? "dark" : "light");
      } catch {
        // The theme still changes for this session.
      }
      return next;
    });
  }

  const applyProgress = useCallback((event: ProgressEvent) => {
    setProgress((current) =>
      current.map((item, index) => {
        if (index < event.index || (index === event.index && event.state === "done")) {
          return { ...item, total: event.total, state: "done" };
        }
        if (index === event.index) {
          return { ...item, total: event.total, state: event.state };
        }
        return { ...item, total: event.total, state: "pending" };
      }),
    );
  }, []);

  async function handleSwitch() {
    if (!canSwitch) return;
    setOperationKind("switch");
    setOperation("running");
    setResult(null);
    setNotice(null);
    setProgress(makeProgress(SWITCH_STEPS[language]));
    setView("activity");
    try {
      const normalizedApiUrl = saveApiUrl(apiUrl);
      const operationResult = await switchProvider(
        normalizedApiUrl,
        apiKey.trim(),
        selectedModel,
        applyProgress,
      );
      setResult(operationResult);
      setOperation("success");
      setStatus((current) =>
        current
          ? {
              ...current,
              currentProvider: "vexlune_hub",
              currentModel: selectedModel,
              currentRoute: "generic",
              configStatus: "normal",
              providerConfigured: true,
              providerRequiresOpenaiAuth: true,
              apiKeyStored: true,
              keyValidationState: operationResult.demoMode ? "stored" : "verified",
              codexRunning: true,
            }
          : current,
      );
      setDesiredRoute("generic");
    } catch (error) {
      const normalized = normalizeError(error);
      setOperation("error");
      setResult({
        success: false,
        route: "Custom API",
        completedAt: new Date().toLocaleString(language === "zh" ? "zh-CN" : "en-US", {
          hour12: false,
        }),
        message:
          language === "zh"
            ? normalized.message
            : ENGLISH_ERRORS[normalized.code] ?? "The operation could not be completed.",
        detail: normalized.rolledBack
          ? text("已恢复操作前状态。", "The previous state was restored.")
          : text("未留下半切换配置。", "No partial route was left behind."),
        errorCode: normalized.code,
        rolledBack: normalized.rolledBack,
        configChanged: normalized.configChanged,
        httpStatus: normalized.httpStatus,
        requestElapsedMs: normalized.requestElapsedMs,
        codexRestored: normalized.codexRestored,
      });
    } finally {
      setApiKey("");
      setShowKey(false);
    }
  }

  function saveModel(model: string) {
    setSelectedModel(model);
    try {
      window.localStorage.setItem("vexlune-hub-model", model);
    } catch {
      // The model remains selected for this session.
    }
  }

  function saveApiUrl(value: string) {
    const normalized = normalizeApiRootInput(value);
    setApiUrl(normalized);
    try {
      window.localStorage.setItem("vexlune-hub-generic-api-root", normalized);
    } catch {
      // The address remains available for this session.
    }
    return normalized;
  }

  function focusReplacementKey() {
    setShowKey(false);
    window.requestAnimationFrame(() => {
      keyInputRef.current?.focus();
      keyInputRef.current?.select();
    });
  }

  async function handleFetchModels(automatic = false) {
    if (
      modelsLoading ||
      !apiUrlReady(apiUrl) ||
      (!apiKey.trim() && !status?.apiKeyStored)
    ) return;
    setModelsLoading(true);
    if (!automatic) setNotice(null);
    try {
      const normalizedApiUrl = saveApiUrl(apiUrl);
      const catalog = await fetchModels(normalizedApiUrl, apiKey.trim());
      setAvailableModels(catalog.models);
      setModelRequestElapsed(catalog.requestElapsedMs);
      const nextModel = catalog.models.includes(selectedModel)
        ? selectedModel
        : catalog.models.includes("gpt-5.6-sol")
          ? "gpt-5.6-sol"
          : catalog.models[0];
      saveModel(nextModel);
      setNotice(
        text(
          `已获取 ${catalog.models.length} 个可用模型。`,
          `Loaded ${catalog.models.length} available models.`,
        ),
      );
    } catch (error) {
      const normalized = normalizeError(error);
      setNotice(
        language === "zh"
          ? `${normalized.code}: ${normalized.message}`
          : `${normalized.code}: ${ENGLISH_ERRORS[normalized.code] ?? "Could not load models."}`,
      );
    } finally {
      setModelsLoading(false);
    }
  }

  useEffect(() => {
    const normalizedApiUrl = normalizeApiRootInput(apiUrl);
    const normalizedKey = apiKey.trim();
    if (!apiUrlReady(normalizedApiUrl) || normalizedKey.length < 8 || modelsLoading) {
      return;
    }
    const signature = `${normalizedApiUrl}\u0000${normalizedKey}`;
    if (signature === lastAutomaticModelRequest.current) return;
    const timer = window.setTimeout(() => {
      lastAutomaticModelRequest.current = signature;
      void handleFetchModels(true);
    }, 700);
    return () => window.clearTimeout(timer);
  }, [apiUrl, apiKey, modelsLoading]);

  async function handleRestore() {
    if (!canRestore) return;
    setOperationKind("restore");
    setOperation("running");
    setResult(null);
    setNotice(null);
    setProgress(makeProgress(RESTORE_STEPS[language]));
    setView("activity");
    try {
      const operationResult = await restoreOfficial(applyProgress);
      setResult(operationResult);
      setOperation("success");
      await refresh();
      setDesiredRoute("official");
    } catch (error) {
      const normalized = normalizeError(error);
      setOperation("error");
      setResult({
        success: false,
        route: "OpenAI",
        completedAt: new Date().toLocaleString(language === "zh" ? "zh-CN" : "en-US", {
          hour12: false,
        }),
        message:
          language === "zh"
            ? normalized.message
            : ENGLISH_ERRORS[normalized.code] ?? "The operation could not be completed.",
        detail: text("恢复未完成。", "The restore did not complete."),
        errorCode: normalized.code,
        rolledBack: normalized.rolledBack,
        configChanged: normalized.configChanged,
        codexRestored: normalized.codexRestored,
      });
    }
  }

  async function handleClearLogs() {
    try {
      await clearLogs();
      setNotice(text("脱敏诊断日志已清理。", "Redacted diagnostic logs were cleared."));
    } catch (error) {
      const normalized = normalizeError(error);
      setNotice(`${normalized.code}: ${normalized.message}`);
    }
  }

  async function handleDeleteKey() {
    if (!status?.apiKeyStored || activeRoute === "generic") return;
    if (!window.confirm(text("确定删除已保存的 API Key？", "Delete the saved API key?"))) {
      return;
    }
    try {
      await deleteSavedKey();
      setStatus((current) =>
        current ? { ...current, apiKeyStored: false, keyValidationState: "missing" } : current,
      );
      setNotice(text("已删除保存的 API Key。", "The saved API key was deleted."));
    } catch (error) {
      const normalized = normalizeError(error);
      setNotice(`${normalized.code}: ${normalized.message}`);
    }
  }

  async function windowAction(action: "minimize" | "maximize" | "close") {
    if (!isTauri) return;
    const appWindow = getCurrentWindow();
    if (action === "minimize") await appWindow.minimize();
    if (action === "maximize") {
      await appWindow.toggleMaximize();
      setMaximized(await appWindow.isMaximized());
    }
    if (action === "close") await appWindow.close();
  }

  const currentRouteName =
    activeRoute === "generic"
      ? text("自定义 API 线路", "Custom API Route")
      : text("OpenAI 官方线路", "Official OpenAI Route");
  const keyState = status?.apiKeyStored
    ? text("已安全保存", "Saved securely")
    : text("等待配置", "Not configured");
  const operationTitle =
    operationKind === "restore"
      ? text("恢复官方线路", "Restore official route")
      : text("切换至自定义 API", "Switch to custom API");
  const operationMessage = useMemo(() => {
    if (!result) return "";
    if (!result.success) return result.message;
    return operationKind === "restore"
      ? text("官方线路已恢复。", "The official route is restored.")
      : text("自定义 API 线路已启用。", "The custom API route is active.");
  }, [operationKind, result, text]);

  return (
    <div className={`hub-shell ${darkMode ? "dark" : ""}`}>
      <aside className="sidebar">
        <div className="brand-lockup">
          <span className="brand-logo"><img src="/vexlune-vh-mark.png" alt="" /></span>
          <span><strong>Vexlune Hub</strong><small>{text("Codex API 路由管理工具", "Codex API routing utility")}</small></span>
        </div>

        <nav className="sidebar-nav" aria-label={text("主导航", "Primary navigation")}>
          <button className={view === "route" ? "active" : ""} onClick={() => setView("route")}>
            <Route size={21} /><span>{text("线路切换", "Route Switcher")}</span>
          </button>
          <button className={view === "configuration" ? "active" : ""} onClick={() => setView("configuration")}>
            <KeyRound size={21} /><span>{text("API 配置", "API Configuration")}</span>
          </button>
          <button className={view === "activity" ? "active" : ""} onClick={() => setView("activity")}>
            {operation === "running" ? <LoaderCircle className="spin" size={21} /> : <Activity size={21} />}
            <span>{text("状态监控", "Status Monitor")}</span>
          </button>
          <button className={view === "settings" ? "active" : ""} onClick={() => setView("settings")}>
            <Settings size={21} /><span>{text("设置中心", "Settings")}</span>
          </button>
          <button className={view === "about" ? "active" : ""} onClick={() => setView("about")}>
            <Info size={21} /><span>{text("关于", "About")}</span>
          </button>
        </nav>

        <div className="sidebar-bottom">
          <button className="hub-promo-link" onClick={() => void openVexluneHub()}>
            <ExternalLink size={16} />
            <span><strong>{text("访问 Vexlune Hub", "Visit Vexlune Hub")}</strong><small>hub.vexlune.com</small></span>
          </button>
          <div className="desktop-status">
            <div><span className={`live-dot ${status?.codexRunning ? "online" : ""}`} /><strong>Codex Desktop</strong></div>
            <b>{status?.codexRunning ? text("已连接", "Connected") : text("未运行", "Not running")}</b>
            <small>{text("与 Codex 保持连接中", "Keeping Codex connected")}</small>
          </div>
          <div className="signature">Vexlune Hub © 2026 Hinln<br />{text("通用版 1.0.0", "Generic 1.0.0")}</div>
        </div>
      </aside>

      <section className="workspace">
        <header className="titlebar" data-tauri-drag-region onDoubleClick={() => void windowAction("maximize")}>
          <span data-tauri-drag-region />
          <div className="title-actions">
            <button className="icon-button" aria-label={text("通知", "Notifications")}><Bell size={18} /></button>
            <button className="icon-button" aria-label={text("切换主题", "Toggle theme")} onClick={toggleTheme}>
              {darkMode ? <Sun size={18} /> : <Moon size={18} />}
            </button>
            <div className="language-toggle" aria-label="Language">
              <button className={language === "en" ? "active" : ""} onClick={() => setAppLanguage("en")}>EN</button>
              <i />
              <button className={language === "zh" ? "active" : ""} onClick={() => setAppLanguage("zh")}>中文</button>
            </div>
            <span className="title-separator" />
            <button className="window-button" aria-label="Minimize" onClick={() => void windowAction("minimize")}><Minus size={16} /></button>
            <button className="window-button" aria-label={maximized ? "Restore" : "Maximize"} onClick={() => void windowAction("maximize")}>
              {maximized ? <Copy size={13} /> : <Square size={13} />}
            </button>
            <button className="window-button close" aria-label="Close" onClick={() => void windowAction("close")}><X size={17} /></button>
          </div>
        </header>

        <main className="content">
          {view === "route" && (
            <section className="route-page home-page">
              <div className="page-title compact-title">
                <div><h1>{text("线路切换", "Route Switcher")}</h1><span>{text("当前使用中", "Active now")}</span></div>
                <p>{text("切换 Codex 请求线路，同时保留 ChatGPT 登录状态", "Switch Codex request routing while keeping your ChatGPT sign-in")}</p>
              </div>

              <div className="home-grid">
                <article className="card routes-card home-switch-card">
                  <div className="card-heading"><strong>{text("选择请求线路", "Choose request route")}</strong><span className="healthy"><i />{text("身份通道保持连接", "Identity stays connected")}</span></div>
                  <div className="route-options">
                    <button className={`route-option home-route-option ${desiredRoute === "official" ? "selected" : ""}`} onClick={() => setDesiredRoute("official")}>
                      {desiredRoute === "official" && <CheckCircle2 className="selection-check" size={20} />}
                      <span className="small-route-icon"><Cloud size={24} /></span><strong>{text("OpenAI 官方 API", "Official OpenAI API")}<em>{text("官方认证", "Official")}</em></strong>
                      <p>{text("使用 ChatGPT 账号认证和官方请求线路", "Use ChatGPT account authentication and the official request route")}</p>
                      {activeRoute === "official" ? <b className="active-route"><i />{text("当前使用中", "Currently active")}</b> : <span className="route-action-label">{text("切换回官方 API", "Switch back to official API")}</span>}
                    </button>
                    <button className={`route-option home-route-option ${desiredRoute === "generic" ? "selected" : ""}`} onClick={() => setDesiredRoute("generic")}>
                      {desiredRoute === "generic" && <CheckCircle2 className="selection-check" size={20} />}
                      <span className="small-route-icon"><Waypoints size={24} /></span><strong>{text("自定义 API", "Custom API")}<em className="purple">{text("通用线路", "Generic")}</em></strong>
                      <p>{text("使用自行填写的兼容 API 地址、密钥和模型", "Use your own compatible API address, key, and model")}</p>
                      {activeRoute === "generic" ? <b className="active-route"><i />{text("当前使用中", "Currently active")}</b> : <span className="route-action-label">{text("切换到自定义 API", "Switch to custom API")}</span>}
                    </button>
                  </div>
                  <div className="home-route-footer">
                    <span><ShieldCheck size={17} />{text("切换只改变模型请求线路，不会修改 ChatGPT 登录态。", "Switching only changes model-request routing and does not modify ChatGPT sign-in.")}</span>
                    {desiredRoute === activeRoute
                      ? <button className="primary-button" disabled><CheckCircle2 size={17} />{text("当前线路", "Current route")}</button>
                      : desiredRoute === "generic"
                        ? (!apiUrlReady(apiUrl) || (!apiKey.trim() && !status?.apiKeyStored))
                          ? <button className="primary-button" onClick={() => setView("configuration")}><KeyRound size={17} />{text("先配置 API", "Configure API")}</button>
                          : <button className="primary-button" disabled={!canSwitch} onClick={() => void handleSwitch()}>{operation === "running" ? <LoaderCircle className="spin" size={17} /> : <Zap size={17} />}{text("切换到自定义 API", "Switch to custom API")}</button>
                        : <button className="primary-button" disabled={!canRestore} onClick={() => void handleRestore()}><RotateCcw size={17} />{text("切换回官方 API", "Switch back to official API")}</button>}
                  </div>
                </article>

                <article className="card login-card home-login-card">
                  <div className="card-heading"><strong>{text("ChatGPT 登录态", "ChatGPT sign-in")}</strong><span className={`account-status ${accountConnected ? "connected" : ""}`}>{accountConnected ? text("已连接", "Connected") : text("未检测", "Not detected")}</span></div>
                  <div className="account-row"><span><UserRound size={23} /></span><strong>{text("ChatGPT 账号", "ChatGPT Account")}</strong></div>
                  <div className="account-meta"><div><CreditCard size={18} /><span>{text("订阅信息", "Subscription")}<strong>{accountConnected ? text("可在 Codex 中查看", "View in Codex") : "—"}</strong></span></div><div><Gauge size={18} /><span>{text("官方额度", "Official usage")}<strong>{accountConnected ? text("可在 Codex 中查看", "View in Codex") : "—"}</strong></span></div><div><ShieldCheck size={18} /><span>{text("登录状态", "Sign-in status")}<strong>{accountConnected ? text("正常", "Healthy") : text("未检测", "Not detected")}</strong></span></div></div>
                  <div className="identity-callout"><Info size={17} />{text("API 线路与 ChatGPT 账号身份相互隔离。", "API routing remains isolated from ChatGPT account identity.")}</div>
                </article>
              </div>
            </section>
          )}

          {view === "configuration" && (
            <section className="route-page api-config-page">
              <div className="page-title">
                <div><h1>{text("API 配置", "API Configuration")}</h1><span>{text("通用版", "Generic")}</span></div>
                <p>{text("填写兼容 API 地址和密钥，系统会自动获取模型", "Enter a compatible API address and key to load models automatically")}</p>
              </div>

              <article className="card configuration-card">
                <div className="config-title"><strong>{text("自定义 API 线路配置", "Custom API Configuration")}</strong><button onClick={() => setView("route")}>{text("返回线路切换", "Back to route switcher")}<ArrowRight size={15} /></button></div>
                <div className="config-fields">
                  <label><span>{text("API 地址（无需填写 /v1）", "API address (omit /v1)")}</span><div className="static-field"><input value={apiUrl} placeholder="https://api.example.com" autoComplete="url" spellCheck={false} onChange={(event) => setApiUrl(event.target.value)} onBlur={(event) => saveApiUrl(event.target.value)} /><Link2 size={16} /></div></label>
                  <label className="key-config"><span>API Key</span><div className="key-field"><input ref={keyInputRef} value={apiKey} type={showKey ? "text" : "password"} placeholder={status?.apiKeyStored ? "••••••••••••••••••••••••" : text("输入 API Key", "Enter API key")} autoComplete="off" spellCheck={false} onChange={(event) => setApiKey(event.target.value)} /><button aria-label={text("显示密钥", "Show key")} onClick={() => setShowKey((value) => !value)}>{showKey ? <EyeOff size={17} /> : <Eye size={17} />}</button></div><button className="edit-key" onClick={() => keyInputRef.current?.focus()}>{text("编辑", "Edit")}</button></label>
                  <label><span>{text("默认模型", "Default model")}</span><div className="model-field"><select value={selectedModel} disabled={modelsLoading} onChange={(event) => saveModel(event.target.value)}>{availableModels.map((model) => <option key={model} value={model}>{model}</option>)}</select></div></label>
                  <button className="model-management" disabled={modelsLoading || !apiUrlReady(apiUrl) || (!apiKey.trim() && !status?.apiKeyStored)} onClick={() => void handleFetchModels()}>{modelsLoading ? <LoaderCircle className="spin" size={15} /> : <RefreshCw size={15} />}{text("刷新可用模型", "Refresh models")}</button>
                </div>
                <div className="config-bottom"><div className="config-health"><span><i />{text("密钥状态", "Key status")}<strong>{keyState}</strong></span><span><RefreshCw size={15} />{text("请求状态", "Request status")}<strong>{modelRequestElapsed !== null ? `${modelRequestElapsed}ms` : result?.requestElapsedMs ? `${result.requestElapsedMs}ms` : text("待检测", "Not tested")}</strong></span><span><ShieldCheck size={15} />{text("身份隔离", "Identity isolation")}<strong>{text("正常", "Healthy")}</strong></span></div>
                  {desiredRoute === "generic" ? <button className="primary-button" disabled={!canSwitch} onClick={() => void handleSwitch()}>{operation === "running" ? <LoaderCircle className="spin" size={17} /> : <Zap size={17} />}{text("应用并切换线路", "Apply and switch")}</button> : canRestore ? <button className="primary-button" onClick={() => void handleRestore()}><RotateCcw size={17} />{text("恢复官方线路", "Restore official route")}</button> : <button className="primary-button" onClick={focusReplacementKey}><KeyRound size={17} />{text("替换 API 密钥", "Replace API key")}</button>}
                </div>
              </article>

              <div className="utility-row"><div><button onClick={() => void openBackups()}><FileClock size={16} />{text("导出配置", "Export config")}</button><button onClick={() => void openBackups()}><FolderOpen size={16} />{text("导入配置", "Import config")}</button><button onClick={() => setDesiredRoute(activeRoute)}><RotateCcw size={16} />{text("重置选择", "Reset selection")}</button></div><div><button onClick={() => setView("activity")}><FileClock size={16} />{text("日志目录", "Logs")}</button><i /><button onClick={() => void openCodexHome()}><FolderOpen size={16} />{text("打开工具目录", "Open tool folder")}</button></div></div>
            </section>
          )}

          {view === "activity" && (
            <section className="secondary-page">
              <div className="secondary-title"><div><h1>{text("状态监控", "Status Monitor")}</h1><p>{text("查看每一次备份、验证、切换与恢复步骤。", "Follow every guarded backup, validation, switch, and restore step.")}</p></div><button className="secondary-button" onClick={() => void refresh()}><RefreshCw size={16} />{text("刷新状态", "Refresh")}</button></div>
              <div className="monitor-grid">
                <article className="card monitor-route-card">
                  <div className="card-heading"><strong><Info size={15} />{text("当前使用线路", "Current route")}</strong><span className="healthy"><i />{text("运行正常", "Healthy")}</span></div>
                  <div className="monitor-route-body"><span className="small-route-icon">{activeRoute === "official" ? <Cloud size={24} /> : <Waypoints size={24} />}</span><div><strong>{currentRouteName}</strong><p>{activeRoute === "official" ? text("ChatGPT 账号认证 · OpenAI API", "ChatGPT authentication · OpenAI API") : `${selectedModel} · VEXLUNE_HUB_API_KEY`}</p></div><button onClick={() => setView("route")}>{text("切换线路", "Switch route")}<ChevronRight size={15} /></button></div>
                </article>
                <article className="card monitor-flow-card">
                  <div className="card-heading"><strong><ShieldCheck size={15} />{text("请求发送流程", "Request flow")}</strong></div>
                  <div className="compact-flow"><span><UserRound size={20} /><b>{text("ChatGPT 身份", "ChatGPT identity")}</b></span><ArrowRight size={17} /><span><Laptop size={20} /><b>Codex</b></span><ArrowRight size={17} /><span>{activeRoute === "official" ? <Cloud size={20} /> : <Waypoints size={20} />}<b>{activeRoute === "official" ? "OpenAI API" : text("自定义 API", "Custom API")}</b></span></div>
                </article>
              </div>
              <div className="secondary-grid">
                <article className="card progress-card"><div className="card-heading"><strong>{operationTitle}</strong><span className={`operation-badge ${operation}`}>{operation === "running" ? text("执行中", "Running") : operation === "success" ? text("已完成", "Complete") : operation === "error" ? text("失败", "Failed") : text("等待操作", "Waiting")}</span></div><ol className="progress-list">{progress.map((item, index) => <li className={item.state} key={`${index}-${item.step}`}><span>{item.state === "done" ? <Check size={14} /> : item.state === "active" ? <LoaderCircle className="spin" size={15} /> : item.state === "error" ? <AlertCircle size={15} /> : <Circle size={13} />}</span><b>{progressLabels[index] ?? item.step}</b><small>{item.state === "done" ? text("完成", "Done") : item.state === "active" ? text("进行中", "Running") : text("等待", "Waiting")}</small></li>)}</ol></article>
                <article className="card result-card"><div className="card-heading"><strong>{text("执行结果", "Operation result")}</strong><Gauge size={19} /></div>{result ? <div className={`result-summary ${result.success ? "success" : "error"}`}>{result.success ? <CheckCircle2 size={29} /> : <AlertCircle size={29} />}<div><strong>{operationMessage}</strong><p>{result.detail}</p></div></div> : <div className="empty-result"><Activity size={31} /><strong>{text("暂无执行记录", "No recent operation")}</strong><p>{text("选择线路并开始操作后，诊断信息会显示在这里。", "Choose a route to see diagnostics here.")}</p></div>}{result && <dl className="result-metadata"><div><dt>{text("完成时间", "Completed")}</dt><dd>{result.completedAt}</dd></div><div><dt>{text("目标线路", "Route")}</dt><dd>{result.route}</dd></div>{result.httpStatus !== undefined && <div><dt>HTTP</dt><dd>{result.httpStatus}</dd></div>}{result.requestElapsedMs !== undefined && <div><dt>{text("耗时", "Elapsed")}</dt><dd>{result.requestElapsedMs}ms</dd></div>}</dl>}<div className="panel-buttons"><button onClick={() => void openBackups()}><FileClock size={16} />{text("打开备份", "Open backups")}</button><button onClick={() => setView("route")}><Route size={16} />{text("返回线路切换", "Back to route switcher")}</button></div></article>
              </div>
            </section>
          )}

          {view === "settings" && (
            <section className="secondary-page">
              <div className="secondary-title"><div><h1>{text("设置中心", "Settings")}</h1><p>{text("动态 Provider、安全状态与本地维护工具。", "Dynamic Provider, security status, and local maintenance.")}</p></div></div>
              <div className="secondary-grid settings-layout">
                <article className="card settings-card"><div className="card-heading"><strong><Network size={17} />{text("自定义线路配置", "Custom route configuration")}</strong></div><dl className="settings-list"><div><dt>{text("API 根地址", "API root")}</dt><dd>{apiUrl || "—"}</dd></div><div><dt>{text("实际 Base URL", "Effective Base URL")}</dt><dd>{apiUrlReady(apiUrl) ? `${normalizeApiRootInput(apiUrl)}/v1` : "—"}</dd></div><div><dt>{text("默认模型", "Default model")}</dt><dd>{selectedModel}</dd></div><div><dt>{text("可用模型", "Available models")}</dt><dd>{availableModels.length}</dd></div><div><dt>{text("环境变量", "Environment key")}</dt><dd>VEXLUNE_HUB_API_KEY</dd></div><div><dt>Wire API</dt><dd>responses</dd></div><div><dt>requires_openai_auth</dt><dd>{status?.providerRequiresOpenaiAuth === true ? "true" : "—"}</dd></div><div><dt>{text("备份数量", "Backups")}</dt><dd>{status?.backupCount ?? "—"}</dd></div></dl></article>
                <article className="card settings-card"><div className="card-heading"><strong><ShieldCheck size={17} />{text("账号连续性与维护", "Identity continuity & maintenance")}</strong></div><div className="privacy-note"><ShieldCheck size={23} /><p>{text("应用只检查 auth.json 是否存在，不会读取、复制、删除或修改其内容。ChatGPT 身份与模型请求凭据始终分离。", "The app only checks whether auth.json exists. It never reads, copies, deletes, or modifies it. ChatGPT identity remains separate from model-request credentials.")}</p></div><div className="maintenance-list"><button onClick={() => void openCodexHome()}><span><FolderOpen size={18} /><b>{text("Codex 配置目录", "Codex directory")}</b></span><ChevronRight size={17} /></button><button onClick={() => void openBackups()}><span><FileClock size={18} /><b>{text("事务备份", "Transaction backups")}</b></span><ChevronRight size={17} /></button><button onClick={() => void handleClearLogs()}><span><RotateCcw size={18} /><b>{text("清理脱敏日志", "Clear redacted logs")}</b></span><ChevronRight size={17} /></button><button className="danger" disabled={!status?.apiKeyStored || activeRoute === "generic"} onClick={() => void handleDeleteKey()}><span><Trash2 size={18} /><b>{text("删除已保存 API Key", "Delete saved API key")}</b></span><ChevronRight size={17} /></button></div></article>
              </div>
            </section>
          )}

          {view === "about" && (
            <section className="secondary-page about-page"><div className="secondary-title"><div><h1>{text("关于", "About")}</h1><p>CodexGo by Vexlune Hub</p></div></div><article className="card about-card"><span className="about-logo"><img src="/vexlune-vh-mark.png" alt="" /></span><h2>CodexGo</h2><p>{text("由 Vexlune Hub 开发的 Codex Desktop 通用 API 路由管理工具。", "A generic Codex Desktop API routing utility by Vexlune Hub.")}</p><dl><div><dt>{text("版本", "Version")}</dt><dd>1.0.0</dd></div><div><dt>{text("品牌 / 开发者", "Brand / Developer")}</dt><dd>Vexlune Hub · Hinln</dd></div><div><dt>{text("兼容接口", "Compatible API")}</dt><dd>OpenAI-compatible /v1</dd></div></dl><button className="about-hub-link" onClick={() => void openVexluneHub()}><ExternalLink size={17} />{text("访问 Vexlune Hub 中转站", "Visit Vexlune Hub")}</button><div className="about-safety"><ShieldCheck size={21} />{text("线路切换只改变模型请求路由，保留 ChatGPT 登录状态与现有会话。", "Route switching changes only model-request routing and preserves ChatGPT sign-in and existing conversations.")}</div><small>{text("独立工具，与 OpenAI 无隶属关系。", "Independent utility. Not affiliated with OpenAI.")}</small></article></section>
          )}

          {notice && <div className="global-notice" role="status"><Info size={17} /><span>{notice}</span><button aria-label={text("关闭", "Dismiss")} onClick={() => setNotice(null)}><X size={15} /></button></div>}
        </main>
      </section>
    </div>
  );
}
