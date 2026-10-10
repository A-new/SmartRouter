// Tauri IPC API layer — replaces window.electron.* calls
// All frontend↔backend communication goes through this module.
//
// Trimmed to the Free Models + Smart Router feature set. Domain modules are
// re-exported so consumers can keep using `import * as api from '../api/tauri'`.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { AppSettings } from './types';

// ─── Re-export domain modules ───

export * from './models';
export * from './secret';
export * from './freeModels';
export * from './smartRouter';

// ─── In-app self-update (Windows): download installer, launch it, exit ───

export interface SelfUpdateProgress {
  status: 'speed_test' | 'downloading' | 'launching' | 'error';
  percent: number;
}

/// Windows-only. Downloads the installer from GitHub releases, launches its
/// wizard, then exits so the installer can replace our files.
/// Rejects on non-Windows or download failure — the caller falls back to
/// opening the download page in the browser.
export async function downloadAndInstallUpdate(version: string): Promise<void> {
  return invoke('download_and_install_update', { version });
}

/// Subscribe to self-update progress while downloadAndInstallUpdate runs.
export function onSelfUpdateProgress(
  callback: (data: SelfUpdateProgress) => void
): Promise<UnlistenFn> {
  return listen<SelfUpdateProgress>('self-update-progress', (event) => {
    callback(event.payload);
  });
}

// ─── Shell APIs (uses Tauri shell plugin) ───

export async function openExternal(url: string): Promise<void> {
  const { open } = await import('@tauri-apps/plugin-shell');
  await open(url);
}

// ─── App Settings APIs ───

export async function getSettings(): Promise<AppSettings> {
  return invoke('get_settings');
}

export async function saveSettings(settings: AppSettings): Promise<void> {
  return invoke('save_settings', { settings });
}

// ─── App Lifecycle APIs ───

export async function appReady(): Promise<void> {
  return invoke('app_ready');
}

// ─── Window APIs (Tauri built-in) ───

export { getCurrentWindow } from '@tauri-apps/api/window';
