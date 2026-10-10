// context.ts — ModelNexus shared context + types (Add/Edit Model modal only)
import React, { createContext, useContext } from 'react';
import type { ModelConfig } from '../../api/types';

// ===== Types =====

export interface NewModelForm {
  name: string;
  baseUrl: string;
  anthropicUrl: string;
  apiKey: string;
  modelId: string;
  // Smart Router logical-model group. Candidates sharing a group fail over to
  // each other, and the group name is callable as a `model` on the router.
  routerGroup: string;
  // Requests-per-minute budget the router may spend on this model's upstream
  // account, kept as text so the input can stay blank. Blank (or `0`) means "no
  // declaration" — the router then learns the limit from response headers, while
  // `-1` (api.ROUTER_RPM_UNLIMITED) declares a paid key with no rate limit.
  routerRpm: string;
  // Quick-pick model id options, carried over from a right-panel directory
  // entry click. ≥2 options → the Add-Model modal shows a dropdown above the
  // model id input. Absent/empty → no dropdown (manual add / edit). Purely a
  // convenience picker; the model id input is always free-editable.
  modelIdOptions?: string[];
}

export type ModelModalDestination = 'modelNexus' | 'freeRouter';

export interface ModelNexusCtx {
  // Models
  userModels: ModelConfig[];
  setUserModels: React.Dispatch<React.SetStateAction<ModelConfig[]>>;
  isLoadingModels: boolean;
  // Modal
  showAddModelModal: boolean;
  setShowAddModelModal: (v: boolean) => void;
  modelModalDestination: ModelModalDestination;
  setModelModalDestination: (destination: ModelModalDestination) => void;
  modelModalAnimatingOut: boolean;
  editingModelId: string | null;
  setEditingModelId: (v: string | null) => void;
  newModelForm: NewModelForm;
  setNewModelForm: React.Dispatch<React.SetStateAction<NewModelForm>>;
  keyDestroyed: boolean;
  setKeyDestroyed: (v: boolean) => void;
  closeModelModal: () => void;
}

// ===== Context =====

export const ModelNexusContext = createContext<ModelNexusCtx | null>(null);

export const useModelNexus = () => {
  const ctx = useContext(ModelNexusContext);
  if (!ctx) throw new Error('useModelNexus must be used within ModelNexusProvider');
  return ctx;
};
