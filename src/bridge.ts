import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  DetectionStatus,
  ModelCatalog,
  OperationResult,
  ProgressEvent,
} from "./types";

export const isTauri = "__TAURI_INTERNALS__" in window;

const demoStatus: DetectionStatus = {
  codexHome: "C:\\Users\\User\\.codex",
  codexDetected: true,
  authPresent: true,
  configPresent: true,
  sessionsPresent: true,
  accountConnected: true,
  currentProvider: "openai",
  currentModel: "gpt-5.6-sol",
  currentRoute: "official",
  configStatus: "normal",
  providerConfigured: true,
  providerRequiresOpenaiAuth: true,
  apiKeyStored: true,
  keyValidationState: "stored",
  codexRunning: true,
  backupCount: 3,
  recoveryPending: false,
};

function wait(ms: number) {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

export async function detectStatus(): Promise<DetectionStatus> {
  if (!isTauri) {
    await wait(180);
    return demoStatus;
  }
  return invoke<DetectionStatus>("detect_status");
}

export async function switchProvider(
  apiUrl: string,
  apiKey: string,
  model: string,
  onProgress: (event: ProgressEvent) => void,
): Promise<OperationResult> {
  if (!isTauri) {
    const labels = [
      "检测 Codex 状态",
      "关闭 Codex",
      "备份配置",
      "验证 API Key",
      "更新 Provider",
      "连接双通道",
      "验证配置",
      "启动 Codex",
      "完成",
    ];
    for (let index = 0; index < labels.length; index += 1) {
      onProgress({
        step: labels[index],
        index,
        total: labels.length,
        state: "active",
      });
      await wait(index === 0 ? 500 : 260);
      onProgress({
        step: labels[index],
        index,
        total: labels.length,
        state: "done",
      });
    }
    return {
      success: true,
      route: "Custom API",
      baseUrl: `${apiUrl.replace(/\/+$/, "")}/v1`,
      completedAt: new Date().toLocaleString("en-US", { hour12: false }),
      message: "The custom API route preview is active.",
      detail: "Demo preview only. No live API request was sent.",
      migrationCount: 1,
      demoMode: true,
    };
  }

  let unlisten: UnlistenFn | undefined;
  try {
    unlisten = await listen<ProgressEvent>("switch-progress", (event) => {
      onProgress(event.payload);
    });
    return await invoke<OperationResult>("switch_provider", {
      request: { apiUrl, apiKey, model },
    });
  } finally {
    unlisten?.();
  }
}

export async function fetchModels(apiUrl: string, apiKey: string): Promise<ModelCatalog> {
  if (!isTauri) {
    await wait(420);
    return {
      models: ["gpt-5.6-sol", "gpt-5.6-terra", "custom-model"],
      httpStatus: 200,
      requestElapsedMs: 320,
    };
  }
  return invoke<ModelCatalog>("fetch_models", {
    request: { apiUrl, apiKey },
  });
}

export async function restoreOfficial(
  onProgress: (event: ProgressEvent) => void,
): Promise<OperationResult> {
  if (!isTauri) {
    const labels = [
      "检测 Codex 状态",
      "关闭 Codex",
      "备份当前配置",
      "读取恢复基线",
      "恢复 Provider",
      "保留自定义 API 密钥",
      "验证恢复结果",
      "启动 Codex",
      "完成",
    ];
    for (let index = 0; index < labels.length; index += 1) {
      onProgress({
        step: labels[index],
        index,
        total: labels.length,
        state: "active",
      });
      await wait(180);
      onProgress({
        step: labels[index],
        index,
        total: labels.length,
        state: "done",
      });
    }
    return {
      success: true,
      route: "Official Codex",
      completedAt: new Date().toLocaleString("en-US", { hour12: false }),
      message: "The original Codex provider preview was restored.",
      detail: "Demo preview only. The saved custom API key remains available.",
      rolledBack: false,
      configChanged: true,
      demoMode: true,
    };
  }
  let unlisten: UnlistenFn | undefined;
  try {
    unlisten = await listen<ProgressEvent>("switch-progress", (event) => {
      onProgress(event.payload);
    });
    return await invoke<OperationResult>("restore_official");
  } finally {
    unlisten?.();
  }
}

export async function openCodexHome() {
  if (isTauri) await invoke("open_codex_home");
}

export async function openBackups() {
  if (isTauri) await invoke("open_backup_directory");
}

export async function openVexluneHub() {
  if (isTauri) {
    await invoke("open_vexlune_hub");
  } else {
    window.open("https://hub.vexlune.com", "_blank", "noopener,noreferrer");
  }
}

export async function clearLogs() {
  if (isTauri) await invoke("clear_logs");
}

export async function deleteSavedKey() {
  if (isTauri) {
    await invoke("delete_saved_key");
  }
}
