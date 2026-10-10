// Model Nexus — shared provider state + Add/Edit Model modal
// Trimmed to the pieces the Free Models + Smart Router feature needs.

import { useState, useEffect, useCallback } from 'react';
import { readText as readClipboardText } from '@tauri-apps/plugin-clipboard-manager';
import { X, Lock, Unlock } from 'lucide-react';
import { ModelIdCombobox } from '../../components/ModelIdCombobox';
import { useToast } from '../../components/Toast';
import { useI18n } from '../../hooks/useI18n';
import * as api from '../../api/tauri';
import type { ModelConfig } from '../../api/types';
import { normalizeAnthropicUrl, normalizeOpenaiUrl } from '../../utils/normalizeUrl';
import { ModelNexusContext, useModelNexus } from './context';
import type { NewModelForm } from './context';
import { useFreeModels } from '../FreeModels/FreeModels';

const visibleModelNexusModels = (models: ModelConfig[]) =>
  models.filter(
    (model) => model.internalId !== 'local-server' && model.internalId !== 'smart-router'
  );

const isValidModelBaseUrl = (value: string) => {
  try {
    const url = new URL(value);
    return (url.protocol === 'http:' || url.protocol === 'https:') && Boolean(url.host);
  } catch {
    return false;
  }
};

// ===== Provider =====

export function ModelNexusProvider({ children }: { children: React.ReactNode }) {
  // Models state
  const [userModels, setUserModels] = useState<ModelConfig[]>([]);
  const [isLoadingModels, setIsLoadingModels] = useState(true);

  // Modal state
  const [showAddModelModal, setShowAddModelModal] = useState(false);
  const [modelModalDestination, setModelModalDestination] = useState<'modelNexus' | 'freeRouter'>(
    'modelNexus'
  );
  const [modelModalAnimatingOut, setModelModalAnimatingOut] = useState(false);
  const [editingModelId, setEditingModelId] = useState<string | null>(null);
  const [keyDestroyed, setKeyDestroyed] = useState(false);
  const [newModelForm, setNewModelForm] = useState<NewModelForm>({
    name: '',
    baseUrl: '',
    anthropicUrl: '',
    apiKey: '',
    modelId: '',
    routerGroup: '',
    routerRpm: '',
  });

  const closeModelModal = useCallback(() => {
    setModelModalAnimatingOut(true);
    setTimeout(() => {
      setModelModalAnimatingOut(false);
      setShowAddModelModal(false);
      setEditingModelId(null);
      setModelModalDestination('modelNexus');
    }, 200);
  }, []);

  // Load models from config
  useEffect(() => {
    const loadModels = async () => {
      setIsLoadingModels(true);
      if (api.getModels) {
        try {
          const models = await api.getModels();
          setUserModels(visibleModelNexusModels(models));
        } catch (error) {
          console.error('Load models failed:', error);
        }
      }
      setIsLoadingModels(false);
    };
    loadModels();
  }, []);

  // Auto-fill Model ID and API Key for local models. Derived during render
  // rather than in an effect: the condition is idempotent (it stops firing once
  // the fields are filled), so React can settle it in a single extra pass
  // instead of committing a paint and then cascading a second render.
  const isLocalUrl = (url: string) => url.includes('localhost') || url.includes('127.0.0.1');
  if (isLocalUrl(newModelForm.baseUrl) || isLocalUrl(newModelForm.anthropicUrl)) {
    const updates: { modelId?: string; apiKey?: string } = {};
    if (!newModelForm.modelId) updates.modelId = 'local-model';
    if (!newModelForm.apiKey) updates.apiKey = 'not-needed';
    if (Object.keys(updates).length > 0) {
      setNewModelForm((prev) => ({ ...prev, ...updates }));
    }
  }

  return (
    <ModelNexusContext.Provider
      value={{
        userModels,
        setUserModels,
        isLoadingModels,
        showAddModelModal,
        setShowAddModelModal,
        modelModalDestination,
        setModelModalDestination,
        modelModalAnimatingOut,
        editingModelId,
        setEditingModelId,
        newModelForm,
        setNewModelForm,
        keyDestroyed,
        setKeyDestroyed,
        closeModelModal,
      }}
    >
      {children}
    </ModelNexusContext.Provider>
  );
}

// ===== Add/Edit Model Modal =====

export function AddModelModal() {
  const { t } = useI18n();
  const { showToast } = useToast();
  const [isSavingModel, setIsSavingModel] = useState(false);
  const { addSelectedModel, selectedIds } = useFreeModels();
  const {
    showAddModelModal,
    modelModalAnimatingOut,
    editingModelId,
    setEditingModelId,
    newModelForm,
    setNewModelForm,
    keyDestroyed,
    closeModelModal,
    setUserModels,
    setShowAddModelModal,
    modelModalDestination,
    setModelModalDestination,
  } = useModelNexus();

  if (!showAddModelModal) return null;

  // One-click paste affordance for the modal's free-text fields. Plain text,
  // no button styling / hover effect — the label never changes. The `normalize`
  // arg lets a URL field run the pasted text through the SAME normalization the
  // typing path uses, so a pasted full endpoint (`/v1/chat/completions`) is
  // trimmed to a clean base instead of silently keeping a doubled path that
  // would 404 every request while manually typed URLs work.
  //
  // These URL fields have NO overlay control to their right (unlike the API
  // key field, which has the encrypt/decrypt lock button at right-2), so the
  // paste label sits flush at the right edge — right-2 — and the input keeps
  // enough right padding (pr-16) to clear the widest locale's "貼り付け" text.
  const pasteButton = (field: 'baseUrl' | 'anthropicUrl', normalize: (v: string) => string) => (
    <button
      type="button"
      onClick={async () => {
        try {
          const text = (await readClipboardText()).trim();
          if (text) {
            const v = normalize(text);
            setNewModelForm((prev) => ({ ...prev, [field]: v }));
          }
        } catch {
          /* clipboard empty / unreadable — no-op */
        }
      }}
      className="absolute right-2 top-1/2 -translate-y-1/2 cursor-pointer text-xs text-cyber-text-secondary"
    >
      {t('model.paste')}
    </button>
  );

  return (
    <div
      className={`fixed inset-0 z-[9998] flex items-center justify-center transition-all duration-200 ${modelModalAnimatingOut ? 'opacity-0' : 'opacity-100'}`}
      onKeyDown={(e) => {
        if (e.key === 'Escape') closeModelModal();
      }}
    >
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={closeModelModal} />

      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="model-config-dialog-title"
        className={`relative w-[450px] max-w-[90vw] max-h-[90vh] overflow-y-auto border border-cyber-border/30 bg-cyber-surface shadow-2xl rounded-xl transition-all duration-200 ${modelModalAnimatingOut ? 'scale-95 opacity-0' : 'scale-100 opacity-100'}`}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Top accent line */}
        <div className="h-px w-full bg-cyber-border" />

        {/* Header */}
        <div className="px-6 pt-5 pb-4 flex items-center justify-between">
          <div className="flex items-center gap-2">
            <span className="text-cyber-text font-mono text-sm opacity-60">&gt;_</span>
            <span id="model-config-dialog-title" className="text-base font-bold text-cyber-text">
              {editingModelId
                ? t('model.editConfig')
                : modelModalDestination === 'freeRouter'
                  ? t('freeModels.addToRouter')
                  : t('btn.addModel')}
            </span>
          </div>
          <button
            onClick={closeModelModal}
            aria-label={t('btn.close')}
            className="text-cyber-text-secondary hover:text-cyber-text transition-colors"
          >
            <X size={18} />
          </button>
        </div>

        {/* Form */}
        <div className="px-5 pb-5">
          <div className="space-y-4">
            <div>
              <label className="block text-xs text-cyber-text-secondary mb-1">
                {t('model.name')}
              </label>
              <input
                type="text"
                placeholder="e.g. My Model"
                value={newModelForm.name}
                onChange={(e) => setNewModelForm((prev) => ({ ...prev, name: e.target.value }))}
                className="w-full bg-cyber-input border border-cyber-border px-2 py-1.5 text-xs text-cyber-text font-mono focus:border-cyber-border focus:outline-none rounded-button"
              />
            </div>
            <div>
              <label className="block text-xs text-cyber-text-secondary mb-1">
                {t('model.openaiUrl')}
              </label>
              <div className="relative">
                <input
                  type="text"
                  placeholder="https://x.x.com/v1"
                  value={newModelForm.baseUrl}
                  onChange={(e) =>
                    setNewModelForm((prev) => ({
                      ...prev,
                      baseUrl: normalizeOpenaiUrl(e.target.value),
                    }))
                  }
                  className="w-full bg-cyber-input border border-cyber-border px-2 py-1.5 pr-16 text-xs text-cyber-text font-mono focus:border-cyber-border focus:outline-none rounded-button"
                />
                {pasteButton('baseUrl', normalizeOpenaiUrl)}
              </div>
            </div>
            <div>
              <label className="block text-xs text-cyber-text-secondary mb-1">
                {t('model.anthropicUrl')}
              </label>
              <div className="relative">
                <input
                  type="text"
                  placeholder="https://x.x.com/anthropic"
                  value={newModelForm.anthropicUrl}
                  onChange={(e) =>
                    setNewModelForm((prev) => ({
                      ...prev,
                      anthropicUrl: normalizeAnthropicUrl(e.target.value),
                    }))
                  }
                  className="w-full bg-cyber-input border border-cyber-border px-2 py-1.5 pr-16 text-xs text-cyber-text font-mono focus:border-cyber-border focus:outline-none rounded-button"
                />
                {pasteButton('anthropicUrl', normalizeAnthropicUrl)}
              </div>
            </div>
            <div>
              <label className="block text-xs text-cyber-text-secondary mb-1">
                {t('model.modelId')}
              </label>
              {/* Single searchable combobox: free-type any id, with a filtered
                  suggestion dropdown when the clicked directory entry carries a
                  model id list. No options → plain input. */}
              <ModelIdCombobox
                value={newModelForm.modelId}
                onChange={(v) => setNewModelForm((prev) => ({ ...prev, modelId: v }))}
                options={newModelForm.modelIdOptions}
                placeholder={t('model.modelIdPlaceholder')}
                onPaste={async () => {
                  try {
                    const text = (await readClipboardText()).trim();
                    if (text) setNewModelForm((prev) => ({ ...prev, modelId: text }));
                  } catch {
                    /* clipboard empty / unreadable — no-op */
                  }
                }}
                pasteLabel={t('model.paste')}
              />
            </div>
            {modelModalDestination === 'freeRouter' && (
              <div>
                <label className="block text-xs text-cyber-text-secondary mb-1">
                  {t('freeModels.router.group')}
                </label>
                <input
                  type="text"
                  placeholder="claude-sonnet"
                  value={newModelForm.routerGroup}
                  onChange={(e) =>
                    setNewModelForm((prev) => ({ ...prev, routerGroup: e.target.value }))
                  }
                  className="w-full bg-cyber-input border border-cyber-border px-2 py-1.5 text-xs text-cyber-text font-mono focus:border-cyber-border focus:outline-none rounded-button"
                />
                <div className="mt-1 text-[10px] leading-snug text-cyber-text-muted">
                  {t('freeModels.router.groupHint')}
                </div>
              </div>
            )}
            {modelModalDestination === 'freeRouter' && (
              <div>
                <label className="block text-xs text-cyber-text-secondary mb-1">
                  {t('freeModels.router.rpm')}
                </label>
                <input
                  type="text"
                  inputMode="numeric"
                  placeholder="20"
                  value={newModelForm.routerRpm}
                  onChange={(e) =>
                    setNewModelForm((prev) => ({ ...prev, routerRpm: e.target.value }))
                  }
                  className="w-full bg-cyber-input border border-cyber-border px-2 py-1.5 text-xs text-cyber-text font-mono focus:border-cyber-border focus:outline-none rounded-button"
                />
                <div className="mt-1 text-[10px] leading-snug text-cyber-text-muted">
                  {t('freeModels.router.rpmHint')}
                </div>
              </div>
            )}
            <div>
              <label className="block text-xs text-cyber-text-secondary mb-1">
                {t('model.apiKey')}
              </label>
              <div className="relative">
                <input
                  type="text"
                  placeholder="sk-..."
                  value={
                    newModelForm.apiKey.startsWith('enc:v1:')
                      ? '•••••••••••••••'
                      : newModelForm.apiKey
                  }
                  onChange={(e) => setNewModelForm((prev) => ({ ...prev, apiKey: e.target.value }))}
                  className="w-full bg-cyber-input border border-cyber-border px-2 py-1.5 pr-20 text-xs text-cyber-text font-mono focus:border-cyber-border focus:outline-none rounded-button"
                  readOnly={newModelForm.apiKey.startsWith('enc:v1:')}
                />
                {/* One-click paste from clipboard — plain text affordance (no
                    border / button styling, no hover effect, label never
                    changes). Shown only while the key is editable (plaintext);
                    hidden once encrypted (the field is read-only then). */}
                {newModelForm.apiKey !== 'not-needed' &&
                  !newModelForm.apiKey.startsWith('enc:v1:') && (
                    <button
                      type="button"
                      onClick={async () => {
                        try {
                          const text = (await readClipboardText()).trim();
                          if (text) {
                            setNewModelForm((prev) => ({ ...prev, apiKey: text }));
                          }
                        } catch {
                          /* clipboard empty / unreadable — no-op */
                        }
                      }}
                      className="absolute right-8 top-1/2 -translate-y-1/2 cursor-pointer text-xs text-cyber-text-secondary"
                    >
                      {t('model.paste')}
                    </button>
                  )}
                {newModelForm.apiKey !== 'not-needed' && (
                  <button
                    type="button"
                    disabled={!newModelForm.apiKey}
                    onClick={async () => {
                      if (newModelForm.apiKey.startsWith('enc:v1:')) {
                        try {
                          const plain = await api.decryptSecret(newModelForm.apiKey);
                          const newKey = plain || '';
                          setNewModelForm((prev) => ({ ...prev, apiKey: newKey }));
                          if (editingModelId) {
                            setUserModels((prev) =>
                              prev.map((m) =>
                                m.internalId === editingModelId ? { ...m, apiKey: newKey } : m
                              )
                            );
                          }
                        } catch {
                          setNewModelForm((prev) => ({ ...prev, apiKey: '' }));
                        }
                      } else {
                        try {
                          const encrypted = await api.encryptSecret(newModelForm.apiKey);
                          setNewModelForm((prev) => ({ ...prev, apiKey: encrypted }));
                          if (editingModelId) {
                            setUserModels((prev) =>
                              prev.map((m) =>
                                m.internalId === editingModelId ? { ...m, apiKey: encrypted } : m
                              )
                            );
                          }
                        } catch {
                          // stay plaintext on failure
                        }
                      }
                    }}
                    className={`absolute right-2 top-1/2 -translate-y-1/2 transition-colors hover:opacity-80 ${!newModelForm.apiKey ? 'opacity-30 cursor-not-allowed' : ''}`}
                  >
                    {newModelForm.apiKey.startsWith('enc:v1:') ? (
                      <Lock size={14} className="text-cyber-accent" />
                    ) : (
                      <Unlock size={14} className="text-cyber-text/70" />
                    )}
                  </button>
                )}
              </div>
              {/* Encryption hint — always rendered (locale-aware exact
                  height, no residual space) and toggled via visibility
                  so the form doesn't shift on encrypt / destroy. */}
              <div
                aria-hidden={!newModelForm.apiKey.startsWith('enc:v1:')}
                className={`mt-1 text-xs leading-tight ${
                  keyDestroyed ? 'text-red-400' : 'text-cyber-text/60'
                } ${!newModelForm.apiKey.startsWith('enc:v1:') ? 'invisible' : ''}`}
              >
                {keyDestroyed ? t('key.destroyed') : t('key.encrypted')}
              </div>
            </div>
          </div>
        </div>

        {/* Footer buttons */}
        <div className="flex border-t border-cyber-border">
          <button
            onClick={closeModelModal}
            className="flex-1 px-4 py-3 text-[14px] font-semibold text-cyber-text-secondary hover:text-cyber-text hover:bg-cyber-elevated transition-all border-r border-cyber-border"
          >
            {t('model.escCancel')}
          </button>
          <button
            type="button"
            disabled={isSavingModel}
            onClick={async () => {
              if (isSavingModel) return;
              // Either URL is enough: the router speaks both protocols, so a
              // model with only an Anthropic endpoint is a first-class candidate.
              const openaiUrl = newModelForm.baseUrl.trim();
              const anthropicUrl = newModelForm.anthropicUrl.trim();
              if (
                !newModelForm.name.trim() ||
                !newModelForm.modelId.trim() ||
                !newModelForm.apiKey.trim() ||
                (!openaiUrl && !anthropicUrl)
              ) {
                showToast('warning', t('model.requiredFields'));
                return;
              }
              if (openaiUrl && !isValidModelBaseUrl(openaiUrl)) {
                showToast('warning', t('model.invalidOpenaiUrl'));
                return;
              }
              if (anthropicUrl && !isValidModelBaseUrl(anthropicUrl)) {
                showToast('warning', t('model.invalidAnthropicUrl'));
                return;
              }
              const routerGroup = newModelForm.routerGroup.trim();
              // Blank means "no declaration"; `0` is what the backend reads as
              // clearing one, so the two stay interchangeable. `-1` is the other
              // declaration the backend keeps: a paid key with no rate limit.
              const rpmText = newModelForm.routerRpm.trim();
              const routerRpm = rpmText === '' ? 0 : Number(rpmText);
              const rpmUnlimited = routerRpm === api.ROUTER_RPM_UNLIMITED;
              if (
                !Number.isInteger(routerRpm) ||
                (routerRpm < 0 && !rpmUnlimited) ||
                routerRpm > api.MAX_ROUTER_RPM
              ) {
                showToast('warning', t('freeModels.router.rpmInvalid'));
                return;
              }
              if (
                modelModalDestination === 'freeRouter' &&
                api.SMART_ROUTER_RESERVED_MODEL_IDS.some(
                  (reserved) => reserved.toLowerCase() === routerGroup.toLowerCase()
                )
              ) {
                showToast('warning', t('freeModels.router.groupReserved'));
                return;
              }
              if (
                modelModalDestination === 'freeRouter' &&
                !editingModelId &&
                selectedIds.size >= api.SMART_ROUTER_CANDIDATE_LIMIT
              ) {
                showToast('warning', t('freeModels.router.limitReached'));
                return;
              }

              setIsSavingModel(true);
              try {
                if (editingModelId) {
                  const updatedModel = await api.updateModel(editingModelId, {
                    name: newModelForm.name,
                    baseUrl: newModelForm.baseUrl,
                    anthropicUrl: newModelForm.anthropicUrl,
                    apiKey: newModelForm.apiKey,
                    modelId: newModelForm.modelId,
                    routerGroup: newModelForm.routerGroup,
                    routerRpm,
                  });
                  if (updatedModel) {
                    setUserModels((prev) =>
                      prev.map((m) => (m.internalId === editingModelId ? updatedModel : m))
                    );
                  }
                } else {
                  const newModel = await api.addModel({
                    name: newModelForm.name,
                    baseUrl: newModelForm.baseUrl,
                    anthropicUrl: newModelForm.anthropicUrl || undefined,
                    apiKey: newModelForm.apiKey,
                    modelId: newModelForm.modelId,
                    routerGroup: newModelForm.routerGroup,
                    routerRpm,
                    scope: modelModalDestination === 'freeRouter' ? 'smartRouter' : 'modelCenter',
                  });
                  if (modelModalDestination === 'freeRouter') {
                    try {
                      await addSelectedModel({
                        internalId: newModel.internalId,
                        name: newModel.name,
                        baseUrl: newModel.baseUrl,
                        modelId: newModel.modelId ?? newModelForm.modelId,
                      });
                    } catch (error) {
                      await api.deleteModel(newModel.internalId).catch((rollbackError) => {
                        console.error('Rollback smart router model failed:', rollbackError);
                      });
                      throw error;
                    }
                  } else {
                    setUserModels((prev) => [...prev, newModel]);
                  }
                }

                setEditingModelId(null);
                setNewModelForm({
                  name: '',
                  baseUrl: '',
                  anthropicUrl: '',
                  apiKey: '',
                  modelId: '',
                  routerGroup: '',
                  routerRpm: '',
                });
                setShowAddModelModal(false);
                setModelModalDestination('modelNexus');
              } catch (error) {
                console.error('Save model failed:', error);
                showToast('error', t('error.requestFailed'));
              } finally {
                setIsSavingModel(false);
              }
            }}
            className={`flex-1 px-4 py-3 text-[14px] font-semibold transition-all ${
              isSavingModel
                ? 'text-cyber-text-muted cursor-not-allowed'
                : 'text-cyber-text hover:bg-cyber-text/10'
            }`}
          >
            {t('model.enterSave')}
          </button>
        </div>
      </div>
    </div>
  );
}
