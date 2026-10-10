// App.tsx — Tauri application shell.
// Single-page app: Free Models + Smart Router. Window chrome (TitleBar,
// SettingsDialog) is app-level; the rest lives inside the page.

import { useState, useEffect } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { listen } from '@tauri-apps/api/event';
import { IS_MACOS } from './utils/platform';
import { ToastProvider } from './components';
import { TitleBar } from './components/TitleBar';
import { SettingsDialog } from './components/SettingsDialog';
import { useI18n } from './hooks/useI18n';
import {
  FreeModelsProvider,
  FreeModelsTitleActions,
  FreeModelsMain,
  FreeModelsPanel,
} from './pages/FreeModels';
import { ModelNexusProvider, AddModelModal } from './pages/ModelNexus';

function App() {
  const { t, locale, setLocale } = useI18n();
  const [showSettings, setShowSettings] = useState(false);

  // Track maximized state so the rounded-corner shell goes square when the
  // window fills the screen (rounded corners against screen edges look off).
  const [isMaximized, setIsMaximized] = useState(false);
  useEffect(() => {
    const win = getCurrentWindow();
    win
      .isMaximized()
      .then(setIsMaximized)
      .catch(() => {});
    const unlisten = win.onResized(() => {
      win
        .isMaximized()
        .then(setIsMaximized)
        .catch(() => {});
    });
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  // Wire the macOS app-menu Settings item (⌘,) to the same action as the
  // in-window gear button. Windows/Linux have no app menu, so it never fires.
  useEffect(() => {
    const unlisten = listen('menu-open-settings', () => setShowSettings(true));
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  // Mirror isMaximized to <html> so the global #root clip-path in index.css
  // can drop its rounded corners when the window is maximized. Necessary
  // because the clip lives at #root level (covering modal/toast portals
  // outside the .rounded-xl shell) and CSS can't read React state directly.
  useEffect(() => {
    document.documentElement.classList.toggle('window-maximized', isMaximized);
  }, [isMaximized]);

  return (
    <ToastProvider>
      <FreeModelsProvider>
        <ModelNexusProvider>
          <div
            className={`flex flex-col h-screen w-full bg-cyber-bg overflow-hidden ${
              isMaximized || IS_MACOS ? '' : 'rounded-xl'
            }`}
          >
            {/* Title bar */}
            <TitleBar onSettingsClick={() => setShowSettings(true)} />

            <div className="flex flex-1 overflow-hidden text-cyber-text font-sans p-4 gap-0 relative isolate">
              {/* Main content wrapper — transparent against page bg, Claude-style */}
              <div className="flex-1 flex flex-col overflow-hidden">
                {/* Main + Right panel row */}
                <div className="flex-1 flex gap-3 overflow-hidden">
                  <main className="flex-1 flex flex-col overflow-hidden">
                    <section className="flex-1 flex flex-col overflow-hidden pr-2">
                      {/* Page title bar — fixed-height row so the title sits at
                          the same baseline regardless of action-button height */}
                      <div className="mb-5 flex-shrink-0 flex items-center gap-3 h-10">
                        <div className="flex items-baseline gap-3 flex-1 min-w-0">
                          <h2 className="cjk-title flex-shrink-0">{t('page.freeModels')}</h2>
                          <div className="page-kicker truncate" aria-hidden="true">
                            SMART ROUTER
                          </div>
                        </div>
                        <FreeModelsTitleActions />
                      </div>

                      {/* Page content */}
                      <div className="flex-1 overflow-y-auto">
                        <FreeModelsMain />
                      </div>
                    </section>
                  </main>

                  <aside className="w-80 flex flex-col">
                    <FreeModelsPanel />
                  </aside>
                </div>
              </div>
            </div>
          </div>

          {/* Add / edit model modal — shared by the page and its right panel */}
          <AddModelModal />

          {/* Settings dialog */}
          <SettingsDialog
            isOpen={showSettings}
            onClose={() => setShowSettings(false)}
            locale={locale}
            onLocaleChange={setLocale}
          />
        </ModelNexusProvider>
      </FreeModelsProvider>
    </ToastProvider>
  );
}

export default App;
