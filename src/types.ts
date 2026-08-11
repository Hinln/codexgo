export type OperationState = "idle" | "running" | "success" | "error";

export interface DetectionStatus {
  codexHome: string;
  codexDetected: boolean;
  authPresent: boolean;
  configPresent: boolean;
  sessionsPresent: boolean;
  accountConnected: boolean;
  currentProvider: string | null;
  currentModel: string | null;
  currentRoute: "official" | "generic" | "custom" | "unknown";
  configStatus: "normal" | "missing" | "invalid" | "readonly";
  providerConfigured: boolean;
  providerRequiresOpenaiAuth: boolean | null;
  apiKeyStored: boolean;
  keyValidationState: "missing" | "stored" | "verified";
  codexRunning: boolean;
  backupCount: number;
  recoveryPending: boolean;
}

export interface ModelCatalog {
  models: string[];
  httpStatus: number;
  requestElapsedMs: number;
}

export interface ProgressEvent {
  step: string;
  index: number;
  total: number;
  state: "active" | "done" | "pending" | "error";
}

export interface OperationResult {
  success: boolean;
  route: string;
  baseUrl?: string;
  completedAt: string;
  message: string;
  detail: string;
  backupPath?: string;
  migrationCount?: number;
  errorCode?: string;
  rolledBack?: boolean;
  configChanged?: boolean;
  httpStatus?: number;
  requestElapsedMs?: number;
  codexRestored?: boolean;
  demoMode?: boolean;
}

export interface ErrorPayload {
  code: string;
  message: string;
  configChanged: boolean;
  rolledBack: boolean;
  httpStatus?: number;
  requestElapsedMs?: number;
  codexRestored?: boolean;
}
