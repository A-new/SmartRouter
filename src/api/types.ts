// Shared types for Tauri IPC — replaces window.electron types from vite-env.d.ts

// ─── Model Types ───

export interface ModelConfig {
  internalId: string;
  name: string;
  modelId?: string;
  baseUrl: string;
  apiKey: string;
  anthropicUrl?: string;
  modelType?: 'CLOUD' | 'LOCAL' | 'TUNNEL' | 'DEMO';
  openaiTested?: boolean;
  anthropicTested?: boolean;
  openaiLatency?: number;
  anthropicLatency?: number;
  scope?: 'modelCenter' | 'smartRouter';
  /** Smart Router logical-model group; also a callable model name on the router. */
  routerGroup?: string;
  /**
   * Requests-per-minute budget the router may spend on this model's upstream
   * account. Free tiers count per API key, so models sharing a key share it.
   *
   * `ROUTER_RPM_UNLIMITED` (`-1`) marks a paid account the router must not
   * throttle locally. Absent/`0` means "unknown": only an advertised limit or a
   * 429 slows the account down.
   */
  routerRpm?: number;
}

// ─── App Settings Types ───

export interface AppSettings {
  locale?: string;
  themeMode?: 'light' | 'dark';
  closeToTray?: boolean | null; // null = always ask, true = minimize to tray, false = quit directly
  closeWindowBehaviorSet?: boolean; // Track if user has made a choice about close behavior
}
