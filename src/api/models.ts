// Model APIs — CRUD for the Free Models / Smart Router pages.
import { invoke } from '@tauri-apps/api/core';
import type { ModelConfig } from './types';

export async function getModels(): Promise<ModelConfig[]> {
  return invoke('get_models');
}

export async function addModel(input: {
  name: string;
  baseUrl: string;
  apiKey: string;
  anthropicUrl?: string;
  modelId?: string;
  routerGroup?: string;
  /** Requests per minute; `-1` = paid key with no rate limit, omit for unknown. */
  routerRpm?: number;
  scope?: 'modelCenter' | 'smartRouter';
}): Promise<ModelConfig> {
  const result = await invoke<ModelConfig>('add_model', { input });
  window.dispatchEvent(new Event('models-changed'));
  return result;
}

export async function deleteModel(internalId: string): Promise<boolean> {
  const result = await invoke<boolean>('delete_model', { internalId });
  window.dispatchEvent(new Event('models-changed'));
  return result;
}

export async function updateModel(
  internalId: string,
  updates: {
    name?: string;
    baseUrl?: string;
    apiKey?: string;
    anthropicUrl?: string;
    modelId?: string;
    routerGroup?: string;
    /** `0` clears the declared budget; `-1` declares a paid key with no limit. */
    routerRpm?: number;
  }
): Promise<ModelConfig | null> {
  const result = await invoke<ModelConfig | null>('update_model', { internalId, updates });
  window.dispatchEvent(new Event('models-changed'));
  return result;
}
