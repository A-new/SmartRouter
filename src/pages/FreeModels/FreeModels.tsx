import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { open as shellOpen } from '@tauri-apps/plugin-shell';
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  Box,
  Check,
  CircleHelp,
  Copy,
  ExternalLink,
  GripVertical,
  KeyRound,
  Plus,
  RefreshCw,
  Route,
  X,
} from 'lucide-react';
import * as api from '../../api/tauri';
import type { FreeModelDirectory, FreeModelEntry } from '../../api/freeModels';
import { getModelIcon } from '../../utils/modelIcon';
import { useToast } from '../../components/Toast';
import { useI18n } from '../../hooks/useI18n';
import { copyText } from '../../utils/copyText';
import { useNavigationStore } from '../../stores/navigationStore';
import { useModelNexus } from '../ModelNexus/context';
import './FreeModels.css';

const HUB_HEIGHT = 96;
const SCAN_STEP_MS = 230;
const SCAN_COMPLETE_MS = 320;
const ROUTER_ACTIVITY_POLL_MS = 750;
const ROUTER_ACTIVITY_GRACE_MS = 2200;

/** Fixed-footprint columns whose header edge can be dragged to resize. */
type RouterColumn = 'provider' | 'status' | 'groups' | 'rpm';
/** Sortable columns; `modelId` is the flexible one, so it has no stored width. */
type RouterSortKey = 'modelId' | 'provider' | 'status' | 'groups' | 'rpm';

const ROUTER_COLUMNS: readonly RouterColumn[] = ['provider', 'status', 'groups', 'rpm'];
const ROUTER_COLUMN_DEFAULT_WIDTH: Record<RouterColumn, number> = {
  provider: 112,
  status: 64,
  groups: 84,
  rpm: 96,
};
const ROUTER_COLUMN_MIN_WIDTH = 44;
/** Wide enough for a host name, narrow enough to keep the model column usable. */
const ROUTER_COLUMN_MAX_WIDTH = 360;
const ROUTER_COLUMN_STORAGE_KEY = 'echobird.freeRouter.columns';

/** Hub rows that can be copied to the clipboard. */
type RouterCopyField = 'openai' | 'anthropic' | 'key';

interface FreeModelsContextValue {
  catalog: FreeModelDirectory;
  models: FreeModelEntry[];
  customModels: RouteModelNode[];
  selectedIds: Set<string>;
  refreshing: boolean;
  scanProvider: string;
  scanProgress: number;
  routerBaseUrl: string;
  routerApiKey: string;
  routerOnline: boolean;
  refresh: () => Promise<void>;
  addSelectedModel: (model: RouteModelInput) => Promise<void>;
  removeSelectedModel: (id: string) => Promise<void>;
  setModelGroup: (id: string, group: string) => Promise<void>;
  setModelRpm: (id: string, rpm: number) => Promise<void>;
  /** Persist the list order, which is the router's tie-break between equal candidates. */
  reorderSelectedModels: (ids: string[]) => Promise<void>;
}

interface RouteModelInput {
  internalId: string;
  name: string;
  baseUrl: string;
  modelId: string;
  routerGroup?: string;
  routerRpm?: number;
}

interface RouteModelNode {
  id: string;
  internalId: string;
  provider: string;
  modelId: string;
  baseUrl: string;
  /** Stored credential, still encrypted. The edit dialog saves the whole form,
   *  so it has to be prefilled or saving would wipe the key. */
  apiKey?: string;
  anthropicUrl?: string;
  routerGroup?: string;
  /** Declared requests-per-minute budget for this model's account, if any. */
  routerRpm?: number;
}

interface FreeModelProviderGroup {
  id: string;
  name: string;
  baseUrl: string;
  docsUrl: string;
  models: FreeModelEntry[];
}

const FreeModelsContext = createContext<FreeModelsContextValue | null>(null);
const emptyCatalog: FreeModelDirectory = {
  version: 1,
  updatedAt: '',
  models: [],
};
const providerPriority = ['nvidia-nim', 'openrouter'];

function providerRank(model: FreeModelEntry): number {
  const rank = providerPriority.indexOf(model.providerId);
  return rank === -1 ? providerPriority.length : rank;
}

function groupModelsForScan(models: FreeModelEntry[]): FreeModelEntry[][] {
  const groups = new Map<string, FreeModelEntry[]>();
  models.forEach((model) => {
    const id = `${model.providerId}:${model.baseUrl}`;
    const group = groups.get(id);
    if (group) group.push(model);
    else groups.set(id, [model]);
  });
  return [...groups.values()].sort((left, right) => providerRank(left[0]) - providerRank(right[0]));
}

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

export function useFreeModels() {
  const value = useContext(FreeModelsContext);
  if (!value) throw new Error('useFreeModels must be used within FreeModelsProvider');
  return value;
}

const API_KEY_PREVIEW_LEN = 12;

function shortModelName(modelId: string): string {
  const tail = modelId.split('/').pop() || modelId;
  return tail.length > 27 ? `${tail.slice(0, 24)}…` : tail;
}

/** `host[:port]` of an upstream, which is what the rate limit is counted per. */
function hostOf(baseUrl: string): string {
  try {
    return new URL(baseUrl).host || baseUrl;
  } catch {
    return baseUrl;
  }
}

function apiKeyPreview(key: string): string {
  if (!key) return '—';
  return key.length > API_KEY_PREVIEW_LEN ? `${key.slice(0, API_KEY_PREVIEW_LEN)}…` : key;
}

function clampColumnWidth(width: number): number {
  return Math.min(Math.max(Math.round(width), ROUTER_COLUMN_MIN_WIDTH), ROUTER_COLUMN_MAX_WIDTH);
}

/** Column widths survive a restart; a malformed entry falls back to its default. */
function readColumnWidths(): Record<RouterColumn, number> {
  const widths = { ...ROUTER_COLUMN_DEFAULT_WIDTH };
  try {
    const raw = window.localStorage.getItem(ROUTER_COLUMN_STORAGE_KEY);
    if (!raw) return widths;
    const stored = JSON.parse(raw) as Partial<Record<RouterColumn, unknown>>;
    ROUTER_COLUMNS.forEach((column) => {
      const value = stored?.[column];
      if (typeof value === 'number' && Number.isFinite(value)) {
        widths[column] = clampColumnWidth(value);
      }
    });
  } catch {
    // A corrupt entry is not worth failing the page over, so defaults win.
  }
  return widths;
}

export function FreeModelsProvider({ children }: { children: ReactNode }) {
  const { t } = useI18n();
  const { showToast } = useToast();
  const activePage = useNavigationStore((state) => state.activePage);
  const [catalog, setCatalog] = useState<FreeModelDirectory>(emptyCatalog);
  const [customModels, setCustomModels] = useState<RouteModelNode[]>([]);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const [refreshing, setRefreshing] = useState(false);
  const [scanProvider, setScanProvider] = useState('');
  const [scanProgress, setScanProgress] = useState(0);
  const [routerBaseUrl, setRouterBaseUrl] = useState('http://127.0.0.1:53683/v1');
  const [routerApiKey, setRouterApiKey] = useState('');
  const [routerRunning, setRouterRunning] = useState(false);
  const [usableCandidateCount, setUsableCandidateCount] = useState(0);
  const refreshInFlightRef = useRef(false);
  const routerMutationRef = useRef<Promise<void>>(Promise.resolve());
  const selectedIdsRef = useRef<Set<string>>(new Set());
  const routerLoadedRef = useRef(false);
  const models = catalog.models;

  const loadRouterModels = useCallback(async () => {
    const [router, configuredModels] = await Promise.all([
      api.getSmartRouterConfig(),
      api.getSmartRouterCandidates(),
    ]);
    const modelsById = new Map(configuredModels.map((model) => [model.internalId, model]));
    const routeModels = router.candidateIds.flatMap((internalId) => {
      const model = modelsById.get(internalId);
      if (!model?.modelId || !model.baseUrl) return [];
      return [
        {
          id: internalId,
          internalId,
          provider: model.name,
          modelId: model.modelId,
          baseUrl: model.baseUrl,
          anthropicUrl: model.anthropicUrl,
          apiKey: model.apiKey,
          routerGroup: model.routerGroup,
          routerRpm: model.routerRpm,
        },
      ];
    });
    setCustomModels(routeModels);
    const next = new Set(router.candidateIds);
    selectedIdsRef.current = next;
    setSelectedIds((current) => {
      if (current.size === next.size && [...current].every((id) => next.has(id))) {
        return current;
      }
      return next;
    });
    setRouterBaseUrl(router.baseUrl);
    setRouterApiKey(router.apiKey);
    setRouterRunning(router.running);
    setUsableCandidateCount(router.usableCandidateCount);
    routerLoadedRef.current = true;
  }, []);

  useEffect(() => {
    if (routerLoadedRef.current && activePage !== 'freeModels') return;
    void loadRouterModels().catch((error) =>
      console.error('Load smart router config failed:', error)
    );
  }, [activePage, loadRouterModels]);

  // A routed model is edited in the shared model dialog, which saves through
  // its own copy of the model list. Without this the row would keep showing the
  // old values until the page was left and re-entered.
  useEffect(() => {
    const reload = () => {
      void loadRouterModels().catch((error) =>
        console.error('Load smart router config failed:', error)
      );
    };
    window.addEventListener('models-changed', reload);
    return () => window.removeEventListener('models-changed', reload);
  }, [loadRouterModels]);

  const refresh = useCallback(async () => {
    if (refreshInFlightRef.current) return;
    refreshInFlightRef.current = true;
    setCatalog(emptyCatalog);
    setScanProvider('');
    setScanProgress(0);
    setRefreshing(true);
    try {
      const remote = await api.getFreeModelDirectory();
      if (!remote) {
        showToast('warning', t('freeModels.fetchFailed'));
        return;
      }

      const groups = groupModelsForScan(remote.models);
      const revealedModels: FreeModelEntry[] = [];
      for (const [index, group] of groups.entries()) {
        setScanProvider(group[0]?.provider ?? '');
        await wait(SCAN_STEP_MS);
        revealedModels.push(...group);
        setCatalog({ ...remote, models: [...revealedModels] });
        setScanProgress((index + 1) / groups.length);
      }
      await wait(SCAN_COMPLETE_MS);
    } catch (error) {
      console.error('Fetch free model directory failed:', error);
      showToast('error', t('freeModels.fetchFailed'));
    } finally {
      refreshInFlightRef.current = false;
      setScanProvider('');
      setScanProgress(0);
      setRefreshing(false);
    }
  }, [showToast, t]);

  const addSelectedModel = useCallback(async (model: RouteModelInput) => {
    const id = model.internalId;
    const addition = routerMutationRef.current
      .catch(() => undefined)
      .then(() => {
        const candidateIds = [...new Set([...selectedIdsRef.current, id])];
        return api.setSmartRouterCandidates(candidateIds);
      });
    routerMutationRef.current = addition.then(
      () => undefined,
      () => undefined
    );

    const router = await addition;
    if (!router.candidateIds.includes(id)) {
      throw new Error(`Smart Router rejected candidate: ${id}`);
    }

    const nextIds = new Set(router.candidateIds);
    selectedIdsRef.current = nextIds;
    setSelectedIds(nextIds);
    setCustomModels((current) => [
      ...current.filter((entry) => entry.id !== id),
      {
        id,
        internalId: model.internalId,
        provider: model.name,
        modelId: model.modelId,
        baseUrl: model.baseUrl,
        routerGroup: model.routerGroup,
        routerRpm: model.routerRpm,
      },
    ]);
    setRouterBaseUrl(router.baseUrl);
    setRouterApiKey(router.apiKey);
    setRouterRunning(router.running);
    setUsableCandidateCount(router.usableCandidateCount);
  }, []);

  const removeSelectedModel = useCallback(
    async (id: string) => {
      const removal = routerMutationRef.current
        .catch(() => undefined)
        .then(() => api.removeSmartRouterCandidate(id));
      routerMutationRef.current = removal.then(
        () => undefined,
        () => undefined
      );
      try {
        const router = await removal;
        const nextIds = new Set(router.candidateIds);
        selectedIdsRef.current = nextIds;
        setSelectedIds(nextIds);
        setCustomModels((current) => current.filter((model) => model.id !== id));
        setRouterBaseUrl(router.baseUrl);
        setRouterApiKey(router.apiKey);
        setRouterRunning(router.running);
        setUsableCandidateCount(router.usableCandidateCount);
      } catch (error) {
        console.error('Remove smart router candidate failed:', error);
        showToast('error', t('error.requestFailed'));
      }
    },
    [showToast, t]
  );

  const setModelGroup = useCallback(async (id: string, group: string) => {
    const updated = await api.updateModel(id, { routerGroup: group });
    if (!updated) {
      throw new Error(`Model not found: ${id}`);
    }
    setCustomModels((current) =>
      current.map((entry) =>
        entry.id === id ? { ...entry, routerGroup: updated.routerGroup } : entry
      )
    );
  }, []);

  // `0` clears the declared budget, matching how a blank group name clears the
  // group. The router re-reads it from the model config on the next request.
  const setModelRpm = useCallback(async (id: string, rpm: number) => {
    const updated = await api.updateModel(id, { routerRpm: rpm });
    if (!updated) {
      throw new Error(`Model not found: ${id}`);
    }
    setCustomModels((current) =>
      current.map((entry) => (entry.id === id ? { ...entry, routerRpm: updated.routerRpm } : entry))
    );
  }, []);

  // The candidate order is the router's tie-break between candidates that score
  // the same, so dragging a row changes routing, not just the display. The
  // backend echoes the stored order back, which keeps the list in step with it
  // even when an id was dropped on the way.
  const reorderSelectedModels = useCallback(
    async (ids: string[]) => {
      const reorder = routerMutationRef.current
        .catch(() => undefined)
        .then(() => api.setSmartRouterCandidates(ids));
      routerMutationRef.current = reorder.then(
        () => undefined,
        () => undefined
      );
      try {
        const router = await reorder;
        const order = new Map(router.candidateIds.map((id, index) => [id, index]));
        const nextIds = new Set(router.candidateIds);
        selectedIdsRef.current = nextIds;
        setSelectedIds(nextIds);
        setCustomModels((current) =>
          [...current].sort(
            (left, right) =>
              (order.get(left.id) ?? Number.MAX_SAFE_INTEGER) -
              (order.get(right.id) ?? Number.MAX_SAFE_INTEGER)
          )
        );
        setRouterBaseUrl(router.baseUrl);
        setRouterApiKey(router.apiKey);
        setRouterRunning(router.running);
        setUsableCandidateCount(router.usableCandidateCount);
      } catch (error) {
        console.error('Reorder smart router candidates failed:', error);
        showToast('error', t('error.requestFailed'));
      }
    },
    [showToast, t]
  );

  return (
    <FreeModelsContext.Provider
      value={{
        catalog,
        models,
        customModels,
        selectedIds,
        refreshing,
        scanProvider,
        scanProgress,
        routerBaseUrl,
        routerApiKey,
        routerOnline: routerRunning && usableCandidateCount > 0,
        refresh,
        addSelectedModel,
        removeSelectedModel,
        setModelGroup,
        setModelRpm,
        reorderSelectedModels,
      }}
    >
      {children}
    </FreeModelsContext.Provider>
  );
}

export function FreeModelsTitleActions() {
  const { t } = useI18n();
  const [showHelp, setShowHelp] = useState(false);

  useEffect(() => {
    if (!showHelp) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setShowHelp(false);
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [showHelp]);

  return (
    <>
      <button
        type="button"
        onClick={() => setShowHelp(true)}
        className="flex items-center gap-1.5 text-sm font-mono px-3 py-1.5 border border-cyber-border rounded-button text-cyber-text hover:bg-cyber-text/10 transition-colors"
      >
        <CircleHelp size={13} />
        {t('freeModels.help')}
      </button>

      {showHelp && (
        <div className="fixed inset-0 z-[9998] flex items-center justify-center">
          <div
            className="absolute inset-0 bg-black/60 backdrop-blur-sm"
            onClick={() => setShowHelp(false)}
          />
          <div className="relative w-[520px] max-w-[90vw] border border-cyber-border/30 bg-cyber-surface shadow-2xl rounded-xl overflow-hidden">
            <div className="h-px w-full bg-cyber-border" />
            <button
              type="button"
              onClick={() => setShowHelp(false)}
              aria-label={t('btn.close')}
              className="absolute right-5 top-5 text-cyber-text-secondary hover:text-cyber-text transition-colors"
            >
              <X size={18} />
            </button>

            <div className="max-h-[70vh] overflow-y-auto px-6 pt-6 pb-6 pr-14 space-y-4">
              <div className="flex gap-3">
                <Route size={18} className="mt-0.5 flex-shrink-0 text-cyber-accent" />
                <div>
                  <div className="text-sm font-semibold text-cyber-text">
                    {t('freeModels.help.routerTitle')}
                  </div>
                  <p className="mt-1 text-xs leading-5 text-cyber-text-secondary">
                    {t('freeModels.help.routerDesc')}
                  </p>
                  <p className="mt-2 text-xs leading-5 text-cyber-text-secondary">
                    {t('freeModels.help.routerHow')}
                  </p>
                  <ul className="mt-1 space-y-1.5 text-xs leading-5 text-cyber-text-secondary list-disc list-inside">
                    <li>{t('freeModels.help.routerPoint1')}</li>
                    <li>{t('freeModels.help.routerPoint2')}</li>
                    <li>{t('freeModels.help.routerPoint3')}</li>
                    <li>{t('freeModels.help.routerPoint4')}</li>
                  </ul>
                  <p className="mt-2 text-xs leading-5 text-cyber-text-secondary">
                    {t('freeModels.help.routerSummary')}
                  </p>
                </div>
              </div>

              <div className="h-px bg-cyber-border/60" />

              <div className="flex gap-3">
                <KeyRound size={18} className="mt-0.5 flex-shrink-0 text-cyber-accent" />
                <div>
                  <div className="text-sm font-semibold text-cyber-text">
                    {t('freeModels.help.useTitle')}
                  </div>
                  <ol className="mt-1 space-y-1.5 text-xs leading-5 text-cyber-text-secondary list-decimal list-inside">
                    <li>{t('freeModels.help.step1')}</li>
                    <li>{t('freeModels.help.step2')}</li>
                    <li>{t('freeModels.help.step3')}</li>
                  </ol>
                </div>
              </div>
            </div>

            <button
              type="button"
              onClick={() => setShowHelp(false)}
              className="w-full border-t border-cyber-border px-4 py-3 text-[14px] font-semibold text-cyber-text hover:bg-cyber-text/10 transition-colors"
            >
              {t('btn.close')}
            </button>
          </div>
        </div>
      )}
    </>
  );
}

/**
 * One header cell: clicking the label sorts by that column, dragging its right
 * edge resizes it. The model column flexes instead, so it passes no `width`
 * and gets no resize handle.
 */
function RouterColumnHeader({
  column,
  label,
  width,
  align = 'left',
  sort,
  onSort,
  onResize,
  onResetWidth,
}: {
  column: RouterSortKey;
  label: string;
  /** Omitted for the flexible column, which cannot be resized. */
  width?: number;
  align?: 'left' | 'right';
  sort: { key: RouterSortKey; desc: boolean } | null;
  onSort: (key: RouterSortKey) => void;
  onResize?: (width: number) => void;
  onResetWidth?: () => void;
}) {
  const { t } = useI18n();
  const dragRef = useRef<{ x: number; width: number } | null>(null);
  const sorted = sort?.key === column;

  return (
    <span
      className={`flex items-center gap-1 ${
        width === undefined ? 'min-w-0 flex-1' : 'relative shrink-0'
      } ${align === 'right' ? 'justify-end' : ''}`}
      style={width === undefined ? undefined : { width }}
    >
      <button
        type="button"
        onClick={() => onSort(column)}
        title={t('freeModels.router.sortHint')}
        className={`flex min-w-0 items-center gap-0.5 transition-colors hover:text-cyber-text ${
          sorted ? 'text-cyber-text' : ''
        }`}
      >
        <span className="truncate">{label}</span>
        {sorted ? (
          sort?.desc ? (
            <ArrowDown size={9} className="shrink-0" />
          ) : (
            <ArrowUp size={9} className="shrink-0" />
          )
        ) : (
          <ArrowUpDown size={9} className="shrink-0 opacity-0 group-hover/head:opacity-60" />
        )}
      </button>
      {width !== undefined && onResize && (
        <span
          role="separator"
          aria-orientation="vertical"
          aria-label={t('freeModels.router.resizeHint')}
          title={t('freeModels.router.resizeHint')}
          onPointerDown={(event) => {
            event.preventDefault();
            dragRef.current = { x: event.clientX, width };
            event.currentTarget.setPointerCapture(event.pointerId);
          }}
          onPointerMove={(event) => {
            const start = dragRef.current;
            if (!start) return;
            onResize(clampColumnWidth(start.width + (event.clientX - start.x)));
          }}
          onPointerUp={(event) => {
            dragRef.current = null;
            if (event.currentTarget.hasPointerCapture(event.pointerId)) {
              event.currentTarget.releasePointerCapture(event.pointerId);
            }
          }}
          onDoubleClick={() => onResetWidth?.()}
          className="absolute -right-[3px] top-0 h-full w-[7px] cursor-col-resize touch-none select-none after:absolute after:inset-y-1 after:left-[3px] after:w-px after:bg-transparent hover:after:bg-cyber-accent/60"
        />
      )}
    </span>
  );
}

export function FreeModelsMain() {
  const { t } = useI18n();
  const { showToast } = useToast();
  const {
    models: directoryModels,
    customModels,
    selectedIds,
    routerBaseUrl,
    routerApiKey,
    routerOnline,
    removeSelectedModel,
    setModelGroup,
    setModelRpm,
    reorderSelectedModels,
  } = useFreeModels();
  const { setNewModelForm, setEditingModelId, setModelModalDestination, setShowAddModelModal } =
    useModelNexus();
  const selectedModels = useMemo(
    () => customModels.filter((model) => selectedIds.has(model.id)),
    [customModels, selectedIds]
  );
  const routerAnthropicBaseUrl = routerBaseUrl.replace(/\/v1\/?$/, '');
  // Double-clicking a row opens the shared model dialog prefilled, so a routed
  // model can be renamed or repointed without deleting and re-adding it.
  const openModelEditor = useCallback(
    async (model: RouteModelNode) => {
      // The picker is fed the same directory list the add flow offers, so a dead
      // model can be swapped for a live sibling instead of being typed out. The
      // directory is only fetched by an explicit refresh, so load it on demand.
      const remote =
        directoryModels.length > 0 ? null : await api.getFreeModelDirectory().catch(() => null);
      const directory = directoryModels.length > 0 ? directoryModels : (remote?.models ?? []);
      const wanted = model.baseUrl.trim().replace(/\/+$/, '');
      const listed = directory
        .filter((entry) => entry.baseUrl.trim().replace(/\/+$/, '') === wanted)
        .map((entry) => entry.modelId);
      setNewModelForm({
        name: model.provider,
        baseUrl: model.baseUrl,
        anthropicUrl: model.anthropicUrl ?? '',
        // Still encrypted. The dialog saves the form as it stands and the
        // backend stores what it is handed, so blank here would erase the key.
        apiKey: model.apiKey ?? '',
        modelId: model.modelId,
        routerGroup: model.routerGroup ?? '',
        routerRpm: model.routerRpm === undefined ? '' : String(model.routerRpm),
        // The current id leads even when the provider has retired it, so the
        // field never starts on a value missing from its own list.
        modelIdOptions: Array.from(new Set([model.modelId, ...listed])),
      });
      setEditingModelId(model.internalId);
      setModelModalDestination('freeRouter');
      setShowAddModelModal(true);
    },
    [
      directoryModels,
      setEditingModelId,
      setModelModalDestination,
      setNewModelForm,
      setShowAddModelModal,
    ]
  );
  const [copiedField, setCopiedField] = useState<RouterCopyField | null>(null);
  const copyField = useCallback(
    async (field: RouterCopyField, value: string) => {
      if (!value) return;
      const ok = await copyText(value);
      showToast(ok ? 'success' : 'error', t(ok ? 'btn.copied' : 'error.requestFailed'));
      if (!ok) return;
      setCopiedField(field);
      window.setTimeout(
        () => setCopiedField((current) => (current === field ? null : current)),
        1500
      );
    },
    [showToast, t]
  );
  const copyRow = (field: RouterCopyField, display: string, hint: string, value: string) => (
    <button
      type="button"
      onClick={() => void copyField(field, value)}
      disabled={!value}
      title={hint}
      className="mx-auto flex max-w-full items-center justify-center gap-1.5 px-1.5 py-0.5 rounded transition-colors hover:bg-cyber-text/10 disabled:cursor-default disabled:hover:bg-transparent"
    >
      <span className="min-w-0 truncate text-left">{display}</span>
      {copiedField === field ? (
        <Check size={11} className="flex-shrink-0" />
      ) : (
        <Copy size={11} className="flex-shrink-0" />
      )}
    </button>
  );
  const [groupEditId, setGroupEditId] = useState<string | null>(null);
  const [groupDraft, setGroupDraft] = useState('');
  const groupEditIdRef = useRef<string | null>(null);

  const startGroupEdit = (model: RouteModelNode) => {
    groupEditIdRef.current = model.id;
    setGroupDraft(model.routerGroup ?? '');
    setGroupEditId(model.id);
  };

  const cancelGroupEdit = useCallback(() => {
    groupEditIdRef.current = null;
    setGroupEditId(null);
  }, []);

  const [rpmEditId, setRpmEditId] = useState<string | null>(null);
  const [rpmDraft, setRpmDraft] = useState('');
  const rpmEditIdRef = useRef<string | null>(null);

  const startRpmEdit = (model: RouteModelNode) => {
    rpmEditIdRef.current = model.id;
    setRpmDraft(model.routerRpm ? String(model.routerRpm) : '');
    setRpmEditId(model.id);
  };

  const cancelRpmEdit = useCallback(() => {
    rpmEditIdRef.current = null;
    setRpmEditId(null);
  }, []);

  // One place owns the write, so the inline editor and the shortcut that
  // declares an account unlimited fail the same way. `-1` survives the cap:
  // `Math.min` leaves it untouched.
  const saveRpm = useCallback(
    async (id: string, rpm: number) => {
      try {
        await setModelRpm(id, Math.min(rpm, api.MAX_ROUTER_RPM));
      } catch (error) {
        console.error('Update router rpm failed:', error);
        showToast('error', t('error.requestFailed'));
      }
    },
    [setModelRpm, showToast, t]
  );

  const commitRpm = useCallback(
    async (id: string) => {
      if (rpmEditIdRef.current !== id) return;
      rpmEditIdRef.current = null;
      setRpmEditId(null);
      const raw = rpmDraft.trim();
      // Blank clears the budget; `-1` declares a paid account with no rate limit;
      // unparseable input leaves it untouched.
      const parsed = raw === '' ? 0 : Math.floor(Number(raw));
      if (!Number.isFinite(parsed) || (parsed < 0 && parsed !== api.ROUTER_RPM_UNLIMITED)) {
        return;
      }
      await saveRpm(id, parsed);
    },
    [rpmDraft, saveRpm]
  );

  const commitGroup = useCallback(
    async (id: string) => {
      // Enter blurs the input and blur commits again, so the ref makes the first
      // commit — or an Escape cancel — the only one that lands.
      if (groupEditIdRef.current !== id) return;
      groupEditIdRef.current = null;
      setGroupEditId(null);
      try {
        await setModelGroup(id, groupDraft.trim());
      } catch (error) {
        console.error('Update router group failed:', error);
        showToast('error', t('error.requestFailed'));
      }
    },
    [groupDraft, setModelGroup, showToast, t]
  );
  const [routerActivity, setRouterActivity] = useState<api.SmartRouterActivity>({
    candidateId: null,
    active: false,
    sequence: 0,
    updatedAtMs: 0,
    cooling: [],
  });
  const [activityObservedAtMs, setActivityObservedAtMs] = useState(0);

  useEffect(() => {
    let cancelled = false;
    let errorReported = false;
    const pollActivity = async () => {
      try {
        const activity = await api.getSmartRouterActivity();
        if (!cancelled) {
          setRouterActivity(activity);
          setActivityObservedAtMs(Date.now());
        }
      } catch (error) {
        if (!cancelled && !errorReported) {
          errorReported = true;
          console.error('Load smart router activity failed:', error);
        }
      }
    };
    void pollActivity();
    const timer = window.setInterval(() => void pollActivity(), ROUTER_ACTIVITY_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  // Models the ledger is holding back right now, so a row can say why a pool is
  // running on fewer models than it has. The backend already drops expired
  // cooldowns, so this needs no clock of its own.
  const coolingIds = useMemo(
    () => new Set(routerActivity.cooling.map((entry) => entry.candidateId)),
    [routerActivity.cooling]
  );

  const activityIsVisible =
    routerActivity.candidateId !== null &&
    (routerActivity.active ||
      activityObservedAtMs - routerActivity.updatedAtMs < ROUTER_ACTIVITY_GRACE_MS);
  const activeCandidateId = activityIsVisible ? routerActivity.candidateId : null;

  const [columnWidths, setColumnWidths] = useState<Record<RouterColumn, number>>(readColumnWidths);
  useEffect(() => {
    try {
      window.localStorage.setItem(ROUTER_COLUMN_STORAGE_KEY, JSON.stringify(columnWidths));
    } catch {
      // Private mode or a full quota: resizing still works for this session.
    }
  }, [columnWidths]);
  const setColumnWidth = useCallback((column: RouterColumn, width: number) => {
    setColumnWidths((current) =>
      current[column] === width ? current : { ...current, [column]: width }
    );
  }, []);
  const resetColumnWidth = useCallback(
    (column: RouterColumn) => setColumnWidth(column, ROUTER_COLUMN_DEFAULT_WIDTH[column]),
    [setColumnWidth]
  );

  // Ranked so a serving model sorts above one in cooldown, and a cooldown above
  // one that is merely idle.
  const statusRank = useCallback(
    (model: RouteModelNode) =>
      model.id === activeCandidateId ? 0 : coolingIds.has(model.id) ? 1 : 2,
    [activeCandidateId, coolingIds]
  );
  const [sort, setSort] = useState<{ key: RouterSortKey; desc: boolean } | null>(null);
  const toggleSort = (key: RouterSortKey) => {
    setSort((current) =>
      current?.key === key ? { key, desc: !current.desc } : { key, desc: false }
    );
  };
  const rows = useMemo(() => {
    if (!sort) return selectedModels;
    const factor = sort.desc ? -1 : 1;
    const value = (model: RouteModelNode): string | number => {
      switch (sort.key) {
        case 'modelId':
          return shortModelName(model.modelId).toLowerCase();
        case 'provider':
          return `${model.provider} ${hostOf(model.baseUrl)}`.toLowerCase();
        case 'status':
          return statusRank(model);
        case 'groups':
          return (model.routerGroup ?? '').toLowerCase();
        case 'rpm':
          // A paid key has no ceiling, so `∞` belongs at the top of the slowest.
          return model.routerRpm === api.ROUTER_RPM_UNLIMITED
            ? Number.POSITIVE_INFINITY
            : (model.routerRpm ?? 0);
      }
    };
    return [...selectedModels].sort((left, right) => {
      const a = value(left);
      const b = value(right);
      if (a === b) return 0;
      return (a < b ? -1 : 1) * factor;
    });
  }, [selectedModels, sort, statusRank]);

  const [draggingId, setDraggingId] = useState<string | null>(null);
  const [dropTargetId, setDropTargetId] = useState<string | null>(null);
  // Only the grip arms a drag, so text selection and the inline editors keep working.
  const dragArmedRef = useRef<string | null>(null);
  const endDrag = () => {
    dragArmedRef.current = null;
    setDraggingId(null);
    setDropTargetId(null);
  };
  const dropRow = (targetId: string) => {
    const sourceId = draggingId;
    endDrag();
    if (!sourceId || sourceId === targetId) return;
    const ids = rows.map((model) => model.id);
    const from = ids.indexOf(sourceId);
    const to = ids.indexOf(targetId);
    if (from < 0 || to < 0) return;
    ids.splice(to, 0, ...ids.splice(from, 1));
    void reorderSelectedModels(ids);
  };

  return (
    <div className="free-model-router min-h-[620px] px-2 py-1">
      <div className="relative pt-6">
        <div
          className={`free-model-router-hub mx-auto w-[min(270px,80%)] rounded-2xl flex flex-col items-center justify-center text-center px-4 py-2 cursor-default ${
            routerOnline ? 'is-running' : selectedModels.length > 0 ? 'is-unavailable' : ''
          }`}
          style={{ minHeight: HUB_HEIGHT }}
        >
          <div className="w-full space-y-1 text-center text-[10px] font-mono free-model-router-state">
            {copyRow('openai', `OpenAI : ${routerBaseUrl}`, routerBaseUrl, routerBaseUrl)}
            {copyRow(
              'anthropic',
              `Anthropic : ${routerAnthropicBaseUrl}`,
              routerAnthropicBaseUrl,
              routerAnthropicBaseUrl
            )}
            {copyRow(
              'key',
              `${t('model.apiKey')} : ${apiKeyPreview(routerApiKey)}`,
              `${t('model.apiKey')}: ${routerApiKey}`,
              routerApiKey
            )}
          </div>
        </div>

        {selectedModels.length === 0 ? (
          <div className="mt-12 text-center">
            <div className="font-medium text-sm text-cyber-text">
              {t('freeModels.router.emptyTitle')}
            </div>
            <div className="mt-2 text-xs text-cyber-text-muted">
              {t('freeModels.router.emptyDesc')}
            </div>
          </div>
        ) : (
          <div
            className={`free-model-router-list mx-auto mt-6 w-full max-w-[1080px] overflow-hidden rounded-xl ${
              routerOnline ? 'is-running' : 'is-unavailable'
            }`}
          >
            <div className="group/head flex items-center gap-2 border-b border-cyber-border/70 px-3 py-1 text-[9px] font-semibold text-cyber-text-muted">
              <span className="w-4 shrink-0" />
              <RouterColumnHeader
                column="modelId"
                label={t('model.modelId')}
                sort={sort}
                onSort={toggleSort}
              />
              <RouterColumnHeader
                column="provider"
                label={t('freeModels.router.provider')}
                width={columnWidths.provider}
                sort={sort}
                onSort={toggleSort}
                onResize={(width) => setColumnWidth('provider', width)}
                onResetWidth={() => resetColumnWidth('provider')}
              />
              <RouterColumnHeader
                column="status"
                label={t('freeModels.router.status')}
                width={columnWidths.status}
                sort={sort}
                onSort={toggleSort}
                onResize={(width) => setColumnWidth('status', width)}
                onResetWidth={() => resetColumnWidth('status')}
              />
              <RouterColumnHeader
                column="groups"
                label={t('freeModels.router.groups')}
                width={columnWidths.groups}
                align="right"
                sort={sort}
                onSort={toggleSort}
                onResize={(width) => setColumnWidth('groups', width)}
                onResetWidth={() => resetColumnWidth('groups')}
              />
              <RouterColumnHeader
                column="rpm"
                label={t('freeModels.router.rpm')}
                width={columnWidths.rpm}
                align="right"
                sort={sort}
                onSort={toggleSort}
                onResize={(width) => setColumnWidth('rpm', width)}
                onResetWidth={() => resetColumnWidth('rpm')}
              />
              <span className="w-6 shrink-0" />
            </div>
            <ul className="divide-y divide-cyber-border/40">
              {rows.map((model) => {
                const isServing = activeCandidateId === model.id;
                const isCooling = coolingIds.has(model.id);
                const statusLabel = isServing
                  ? t('freeModels.router.statusServing')
                  : isCooling
                    ? t('freeModels.router.statusCooling')
                    : t('freeModels.router.statusIdle');
                const statusClass = isServing
                  ? 'text-cyber-accent'
                  : isCooling
                    ? 'text-cyber-warning'
                    : 'text-cyber-text-muted';
                return (
                  <li
                    key={model.id}
                    draggable
                    onDragStart={(event) => {
                      // Only the grip arms a drag; anything else is a selection
                      // or an inline edit and keeps its normal behaviour.
                      if (dragArmedRef.current !== model.id) {
                        event.preventDefault();
                        return;
                      }
                      event.dataTransfer.effectAllowed = 'move';
                      event.dataTransfer.setData('text/plain', model.id);
                      setDraggingId(model.id);
                      // A manual order replaces whatever the header was sorting by.
                      setSort(null);
                    }}
                    onDragOver={(event) => {
                      if (!draggingId) return;
                      event.preventDefault();
                      event.dataTransfer.dropEffect = 'move';
                      setDropTargetId(model.id);
                    }}
                    onDragLeave={() =>
                      setDropTargetId((current) => (current === model.id ? null : current))
                    }
                    onDrop={(event) => {
                      event.preventDefault();
                      dropRow(model.id);
                    }}
                    onDragEnd={endDrag}
                    className={`group flex items-center gap-2 px-3 py-1.5 text-left hover:bg-cyber-text/5 ${
                      dropTargetId === model.id && draggingId !== model.id
                        ? 'free-model-row-drop'
                        : ''
                    } ${draggingId === model.id ? 'opacity-50' : ''}`}
                  >
                    <span className="relative flex w-4 shrink-0 items-center justify-center">
                      <span className="flex items-center justify-center transition-opacity group-hover:opacity-0">
                        {isServing ? (
                          <span className="free-model-live-dot" aria-hidden="true" />
                        ) : (
                          <span
                            className={`h-1.5 w-1.5 rounded-full ${
                              isCooling ? 'bg-cyber-warning/80' : 'bg-cyber-text-muted/40'
                            }`}
                            aria-hidden="true"
                          />
                        )}
                      </span>
                      <button
                        type="button"
                        aria-label={t('freeModels.router.dragHint')}
                        title={t('freeModels.router.dragHint')}
                        onPointerDown={() => {
                          dragArmedRef.current = model.id;
                        }}
                        onPointerUp={() => {
                          dragArmedRef.current = null;
                        }}
                        className="absolute inset-0 flex cursor-grab items-center justify-center rounded text-cyber-text-muted opacity-0 transition-opacity hover:text-cyber-text focus-visible:opacity-100 active:cursor-grabbing group-hover:opacity-100"
                      >
                        <GripVertical size={12} />
                      </button>
                    </span>
                    <span
                      className="min-w-0 flex-1 truncate text-xs font-semibold text-cyber-text"
                      title={`${model.modelId}\n${t('freeModels.router.modelIdHint')}`}
                      onDoubleClick={() => void openModelEditor(model)}
                    >
                      {shortModelName(model.modelId)}
                    </span>
                    <span
                      className="shrink-0 truncate text-[10px] text-cyber-text-muted"
                      style={{ width: columnWidths.provider }}
                      title={model.baseUrl}
                    >
                      {model.provider} · {hostOf(model.baseUrl)}
                    </span>
                    <span
                      className={`shrink-0 truncate text-[10px] ${statusClass}`}
                      style={{ width: columnWidths.status }}
                    >
                      {statusLabel}
                    </span>
                    <span
                      className="flex shrink-0 justify-end"
                      style={{ width: columnWidths.groups }}
                    >
                      {groupEditId === model.id ? (
                        <input
                          autoFocus
                          value={groupDraft}
                          onChange={(event) => setGroupDraft(event.target.value)}
                          onBlur={() => void commitGroup(model.id)}
                          onKeyDown={(event) => {
                            if (event.key === 'Enter') event.currentTarget.blur();
                            if (event.key === 'Escape') cancelGroupEdit();
                          }}
                          placeholder={t('freeModels.router.group')}
                          className="w-full min-w-0 rounded border border-cyber-border bg-cyber-input px-1 py-0 text-[10px] text-cyber-text"
                        />
                      ) : (
                        <button
                          type="button"
                          onClick={() => startGroupEdit(model)}
                          title={t('freeModels.router.groupHint')}
                          className={`max-w-full shrink-0 truncate rounded px-1 text-[9px] transition-colors ${
                            model.routerGroup
                              ? 'bg-cyber-text/10 text-cyber-text-secondary hover:bg-cyber-text/20'
                              : 'border border-dashed border-cyber-border text-cyber-text-muted opacity-0 group-hover:opacity-100 focus-visible:opacity-100'
                          }`}
                        >
                          {model.routerGroup ?? t('freeModels.router.group')}
                        </button>
                      )}
                    </span>
                    <span
                      className="flex shrink-0 items-center justify-end gap-1"
                      style={{ width: columnWidths.rpm }}
                    >
                      {rpmEditId === model.id ? (
                        <input
                          autoFocus
                          value={rpmDraft}
                          inputMode="numeric"
                          onChange={(event) => setRpmDraft(event.target.value)}
                          onBlur={() => void commitRpm(model.id)}
                          onKeyDown={(event) => {
                            if (event.key === 'Enter') event.currentTarget.blur();
                            if (event.key === 'Escape') cancelRpmEdit();
                          }}
                          placeholder="20"
                          className="w-full min-w-0 max-w-[64px] rounded border border-cyber-border bg-cyber-input px-1 py-0 text-[10px] text-cyber-text"
                        />
                      ) : (
                        <>
                          <button
                            type="button"
                            onClick={() => startRpmEdit(model)}
                            title={t('freeModels.router.rpmHint')}
                            className={`shrink-0 rounded px-1 text-[9px] font-mono transition-colors ${
                              model.routerRpm
                                ? 'bg-cyber-text/10 text-cyber-text-secondary hover:bg-cyber-text/20'
                                : 'border border-dashed border-cyber-border text-cyber-text-muted opacity-0 group-hover:opacity-100 focus-visible:opacity-100'
                            }`}
                          >
                            {model.routerRpm === api.ROUTER_RPM_UNLIMITED
                              ? t('freeModels.router.rpmUnlimited')
                              : model.routerRpm
                                ? `${model.routerRpm} RPM`
                                : t('freeModels.router.rpm')}
                          </button>
                          {model.routerRpm !== api.ROUTER_RPM_UNLIMITED && (
                            <button
                              type="button"
                              onClick={() => void saveRpm(model.id, api.ROUTER_RPM_UNLIMITED)}
                              title={t('freeModels.router.rpmHint')}
                              aria-label={t('freeModels.router.rpmUnlimited')}
                              className="shrink-0 rounded border border-dashed border-cyber-border px-1 text-[9px] leading-4 text-cyber-text-muted opacity-0 transition-colors hover:bg-cyber-text/10 hover:text-cyber-text-secondary focus-visible:opacity-100 group-hover:opacity-100"
                            >
                              ∞
                            </button>
                          )}
                        </>
                      )}
                    </span>
                    <button
                      type="button"
                      onClick={() => void removeSelectedModel(model.id)}
                      className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-cyber-text-muted opacity-0 transition-all hover:bg-cyber-text/10 hover:text-cyber-text group-hover:opacity-100 focus-visible:opacity-100"
                      aria-label={`${t('btn.remove')} ${shortModelName(model.modelId)}`}
                    >
                      <X size={14} />
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </div>
    </div>
  );
}

function FreeModelProviderRow({
  group,
  selected,
  onAdd,
}: {
  group: FreeModelProviderGroup;
  selected: boolean;
  onAdd: () => void;
}) {
  const { t } = useI18n();
  const iconSrc = getModelIcon(group.name, '');
  const hostname = (() => {
    try {
      return new URL(group.baseUrl).hostname;
    } catch {
      return group.baseUrl;
    }
  })();
  const openDocs = () => shellOpen(group.docsUrl).catch(() => window.open(group.docsUrl, '_blank'));

  return (
    <div className="free-model-provider-enter relative flex items-stretch rounded overflow-hidden bg-cyber-surface">
      <button
        type="button"
        onClick={onAdd}
        aria-label={`${t('freeModels.addToRouter')}: ${group.name}`}
        aria-pressed={selected}
        className="group/left flex-1 min-h-[64px] bg-gradient-to-r from-transparent to-transparent hover:from-cyber-text/15 hover:to-transparent transition-[background-image] duration-200"
      />
      <button
        type="button"
        onClick={openDocs}
        aria-label={`${t('freeModels.docs')}: ${group.name}`}
        className="group/right flex-1 min-h-[64px] bg-gradient-to-l from-transparent to-transparent hover:from-cyber-text/15 hover:to-transparent transition-[background-image] duration-200"
      />

      <div className="pointer-events-none absolute inset-0 flex items-center gap-3 px-3">
        {selected ? (
          <Check
            size={22}
            strokeWidth={2.5}
            className="flex-shrink-0 text-cyber-accent group-hover/left:scale-110 transition-all"
          />
        ) : (
          <Plus
            size={22}
            strokeWidth={2.5}
            className="flex-shrink-0 text-cyber-text-muted group-hover/left:text-cyber-text group-hover/left:scale-110 transition-all"
          />
        )}
        <div className="flex-shrink-0">
          {iconSrc ? (
            <img
              src={iconSrc}
              alt=""
              className="w-6 h-6"
              onError={(event) => {
                (event.target as HTMLImageElement).style.display = 'none';
              }}
            />
          ) : (
            <div className="w-6 h-6 flex items-center justify-center text-cyber-text">
              <Box size={22} />
            </div>
          )}
        </div>
        <div className="flex-1 min-w-0 flex flex-col justify-center">
          <div className="text-sm font-bold truncate leading-none">{group.name}</div>
          <div className="text-[10px] text-cyber-text-secondary truncate leading-tight mt-1 opacity-70">
            {hostname}
          </div>
        </div>
        <ExternalLink
          size={18}
          strokeWidth={2.25}
          className="flex-shrink-0 text-cyber-text-muted group-hover/right:text-cyber-text group-hover/right:scale-110 transition-all"
        />
      </div>
    </div>
  );
}

export function FreeModelsPanel() {
  const { t } = useI18n();
  const { models, customModels, selectedIds, refreshing, scanProvider, scanProgress, refresh } =
    useFreeModels();
  const { setNewModelForm, setEditingModelId, setModelModalDestination, setShowAddModelModal } =
    useModelNexus();
  const providerGroups = useMemo(() => {
    const groups = new Map<string, FreeModelProviderGroup>();
    models.forEach((model) => {
      const id = `${model.providerId}:${model.baseUrl}`;
      const group = groups.get(id);
      if (group) group.models.push(model);
      else {
        groups.set(id, {
          id,
          name: model.provider,
          baseUrl: model.baseUrl,
          docsUrl: model.docsUrl,
          models: [model],
        });
      }
    });
    return [...groups.values()].sort((left, right) => {
      return providerRank(left.models[0]) - providerRank(right.models[0]);
    });
  }, [models]);

  const openProvider = useCallback(
    (group: FreeModelProviderGroup) => {
      const modelIds = group.models.map((model) => model.modelId);
      setNewModelForm({
        name: group.name,
        baseUrl: group.baseUrl,
        anthropicUrl: '',
        apiKey: '',
        modelId: modelIds[0] ?? '',
        routerGroup: '',
        // Directory entries are free platforms, whose keys are usually capped
        // around 20 req/min. Prefilled but editable; clearing it means "no
        // declaration" and the router falls back to advertised limits + 429s.
        routerRpm: '20',
        modelIdOptions: modelIds,
      });
      setEditingModelId(null);
      setModelModalDestination('freeRouter');
      setShowAddModelModal(true);
    },
    [setEditingModelId, setModelModalDestination, setNewModelForm, setShowAddModelModal]
  );

  const openCustomModel = useCallback(() => {
    setNewModelForm({
      name: '',
      baseUrl: '',
      anthropicUrl: '',
      apiKey: '',
      modelId: '',
      routerGroup: '',
      routerRpm: '',
    });
    setEditingModelId(null);
    setModelModalDestination('freeRouter');
    setShowAddModelModal(true);
  }, [setEditingModelId, setModelModalDestination, setNewModelForm, setShowAddModelModal]);

  return (
    <div className="free-model-router h-full min-h-0 flex flex-col">
      <div className="h-10 px-2 flex items-center gap-2 bg-transparent">
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={refreshing}
          className={`flex-1 h-9 px-3 text-[14px] font-semibold border rounded-button transition-colors flex items-center justify-center gap-2 ${
            !refreshing
              ? 'border-cyber-border text-cyber-text-secondary hover:text-cyber-text hover:bg-cyber-text/10'
              : 'border-cyber-border text-cyber-text-muted cursor-not-allowed'
          }`}
        >
          <RefreshCw size={14} className={refreshing ? 'animate-spin' : ''} />
          {t('freeModels.fetch')}
        </button>
        <button
          type="button"
          onClick={openCustomModel}
          aria-label={t('freeModels.customAdd')}
          title={t('freeModels.customAdd')}
          className="w-9 h-9 flex items-center justify-center border border-cyber-border rounded-button text-cyber-text hover:bg-cyber-text/10 transition-colors"
        >
          <Plus size={16} />
        </button>
      </div>
      {refreshing && (
        <div className="px-2 pb-2">
          <div className="h-1 rounded-full overflow-hidden bg-cyber-border/30">
            <div
              className="free-model-scan-progress h-full rounded-full bg-cyber-accent"
              style={{ width: `${Math.max(scanProgress, 0.04) * 100}%` }}
            />
          </div>
          <div className="mt-2 text-[11px] text-cyber-text-secondary truncate">
            {scanProvider ? (
              <>
                {t('freeModels.scanning')}
                {scanProvider}
              </>
            ) : (
              <>
                {t('freeModels.scanConnecting')}
                <span className="free-model-scan-dots" aria-hidden="true">
                  <span>.</span>
                  <span>.</span>
                  <span>.</span>
                </span>
              </>
            )}
          </div>
        </div>
      )}
      <div className="flex-1 p-2 overflow-y-auto">
        {providerGroups.length === 0 ? (
          <div className="h-full flex items-center justify-center">
            {!refreshing && (
              <div className="text-xs text-cyber-text-muted text-center">
                {t('freeModels.fetchHint')}
              </div>
            )}
          </div>
        ) : (
          <div className="space-y-2">
            {providerGroups.map((group) => (
              <FreeModelProviderRow
                key={group.id}
                group={group}
                selected={group.models.some((model) =>
                  customModels.some(
                    (routeModel) =>
                      selectedIds.has(routeModel.internalId) &&
                      routeModel.baseUrl === model.baseUrl &&
                      routeModel.modelId === model.modelId
                  )
                )}
                onAdd={() => openProvider(group)}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
