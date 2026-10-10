import { invoke } from '@tauri-apps/api/core';

export type FreeModelType = 'perpetual' | 'renewing-quota' | 'recurring-credit' | 'trial-credit';

export interface FreeModelEntry {
  id: string;
  providerId: string;
  provider: string;
  modelId: string;
  baseUrl: string;
  freeType: FreeModelType;
  freeTier: string;
  rateLimits: string;
  notes: string;
  docsUrl: string;
  cardRequired: boolean | null;
  phoneRequired: boolean | null;
  commercialOk: boolean | null;
  verifiedAt: string;
}

export interface FreeModelDirectory {
  '$schema-comment'?: string;
  version: number;
  updatedAt: string;
  models: FreeModelEntry[];
}

export async function getFreeModelDirectory(): Promise<FreeModelDirectory | null> {
  const result = await invoke<FreeModelDirectory | null>('get_free_model_directory');
  return result && Array.isArray(result.models) ? result : null;
}
