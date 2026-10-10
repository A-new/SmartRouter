import { invoke } from '@tauri-apps/api/core';
import type { ModelConfig } from './types';

export const SMART_ROUTER_CANDIDATE_LIMIT = 20;

/**
 * Mirrors `model_manager::MAX_ROUTER_RPM`, the largest declared requests-per-minute
 * budget the router honours. Anything above is rejected as a bad declaration.
 */
export const MAX_ROUTER_RPM = 100_000;

/**
 * Mirrors `model::ROUTER_RPM_UNLIMITED`. A declared budget of `-1` means the
 * account is on a paid plan with no rate limit: the router applies no cap of its
 * own and ignores the one the provider advertises. Blank is weaker — it means
 * "unknown", so an advertised limit still holds.
 */
export const ROUTER_RPM_UNLIMITED = -1;

/**
 * Model names the router answers itself. A group may not shadow one of these:
 * the auto aliases are matched first, so such a group would be unreachable.
 */
export const SMART_ROUTER_RESERVED_MODEL_IDS = [
  'auto',
  'echobird/auto',
  'echobird-auto',
  'smart-router',
] as const;

export interface SmartRouterConfig {
  candidateIds: string[];
  /** Group names the router also accepts as a `model` value. */
  groups: string[];
  usableCandidateCount: number;
  baseUrl: string;
  modelId: string;
  port: number;
  running: boolean;
  /** Decrypted local router key — clients pointing at the loopback router must send it. */
  apiKey: string;
}

export interface SmartRouterActivity {
  candidateId: string | null;
  active: boolean;
  sequence: number;
  updatedAtMs: number;
  /** Models the ledger is holding back right now, with the cooldown deadline. */
  cooling: SmartRouterCooling[];
}

/** A model the router is temporarily skipping, e.g. after a 429 or a failure. */
export interface SmartRouterCooling {
  candidateId: string;
  cooldownUntilMs: number;
}

export async function getSmartRouterConfig(): Promise<SmartRouterConfig> {
  return invoke('get_smart_router_config');
}

export async function getSmartRouterActivity(): Promise<SmartRouterActivity> {
  return invoke('get_smart_router_activity');
}

export async function setSmartRouterCandidates(candidateIds: string[]): Promise<SmartRouterConfig> {
  const result = await invoke<SmartRouterConfig>('set_smart_router_candidates', { candidateIds });
  window.dispatchEvent(new Event('models-changed'));
  return result;
}

export async function getSmartRouterCandidates(): Promise<ModelConfig[]> {
  return invoke('get_smart_router_candidates');
}

export async function removeSmartRouterCandidate(internalId: string): Promise<SmartRouterConfig> {
  const result = await invoke<SmartRouterConfig>('remove_smart_router_candidate', { internalId });
  window.dispatchEvent(new Event('models-changed'));
  return result;
}
