// Custom frameless window title bar
import React, { useState, useEffect } from 'react';
import { Settings, Minus, Maximize2, Minimize2, X } from 'lucide-react';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import { getSettings, saveSettings } from '../api/tauri';
import { CloseWindowDialog } from './CloseWindowDialog';
import { useI18n } from '../hooks/useI18n';
import { IS_MACOS } from '../utils/platform';

interface TitleBarProps {
  onSettingsClick?: () => void;
}

export const TitleBar: React.FC<TitleBarProps> = ({ onSettingsClick }) => {
  const { t } = useI18n();
  const handleMinimize = () => getCurrentWindow().minimize();
  // One-click hide to tray. Reuses the same getCurrentWindow().hide() call as
  // the X-button's "tray" choice in handleClose, so tray-restore is identical.
  const handleMinimizeToTray = () => getCurrentWindow().hide();
  const [isMaximized, setIsMaximized] = useState(false);
  const [showCloseDialog, setShowCloseDialog] = useState(false);

  useEffect(() => {
    // Sync initial state and listen for resize/maximize events
    getCurrentWindow()
      .isMaximized()
      .then(setIsMaximized)
      .catch(() => {});
    const unlisten = getCurrentWindow().onResized(() => {
      getCurrentWindow()
        .isMaximized()
        .then(setIsMaximized)
        .catch(() => {});
    });
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  const handleMaximize = async () => {
    const win = getCurrentWindow();
    if (isMaximized) {
      // Always restore to default size (1400×900) + center
      await win.unmaximize();
      await win.setSize(new LogicalSize(1400, 900));
      await win.center();
    } else {
      await win.maximize();
    }
  };

  const handleClose = async () => {
    const win = getCurrentWindow();

    // Check user settings for close behavior
    const settings = await getSettings();

    // If user hasn't set their preference yet, or chose "always ask", show dialog
    if (!settings.closeWindowBehaviorSet || settings.closeToTray === null) {
      setShowCloseDialog(true);
      return;
    }

    // User has set a specific preference
    const closeToTray = settings.closeToTray ?? false;

    if (closeToTray) {
      // Hide window to tray instead of closing
      await win.hide();
    } else {
      // Close the window normally
      win.destroy();
    }
  };

  const handleCloseDialogChoice = async (choice: 'direct' | 'tray' | null) => {
    setShowCloseDialog(false);

    if (choice === null) {
      // User cancelled
      return;
    }

    const win = getCurrentWindow();
    const settings = await getSettings();

    // Only persist this choice as the permanent preference during first-time
    // onboarding (the user has never made an explicit close-behavior choice).
    // When the user has explicitly selected "always ask" (closeToTray === null
    // with closeWindowBehaviorSet === true), this dialog is a one-time prompt and
    // must NOT overwrite their setting — otherwise picking "exit" or "tray" here
    // silently cancels "always ask".
    if (!settings.closeWindowBehaviorSet) {
      await saveSettings({
        ...settings,
        closeToTray: choice === 'tray',
        closeWindowBehaviorSet: true,
      });
    }

    // Execute the chosen action (one-time when in "always ask" mode)
    if (choice === 'tray') {
      await win.hide();
    } else {
      win.destroy();
    }
  };

  return (
    <>
      <div
        className="h-8 bg-cyber-bg flex items-center justify-end select-none flex-shrink-0 cursor-default"
        onMouseDown={(e) => {
          // Use startDragging for Linux (WebkitAppRegion doesn't work on Linux GTK)
          // Also works cross-platform as a reliable fallback
          if (e.button === 0 && !(e.target as HTMLElement).closest('button')) {
            e.preventDefault();
            getCurrentWindow()
              .startDragging()
              .catch(() => {});
          }
        }}
      >
        {/* Window controls */}
        <div
          className="flex items-center h-full"
          style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
        >
          <button
            onClick={onSettingsClick}
            aria-label={t('settings.title')}
            className="h-full px-4 flex items-center justify-center text-cyber-text-secondary hover:bg-cyber-text/20 hover:text-cyber-text transition-colors"
          >
            <Settings size={13} />
          </button>
          <button
            onClick={handleMinimizeToTray}
            aria-label={t('settings.closeToTray')}
            className="h-full px-4 flex items-center justify-center hover:bg-cyber-text/20 transition-colors"
          >
            <img src="/brand/bird.png" alt="" draggable={false} className="h-4 w-4 select-none" />
          </button>
          {/* Window controls — Windows/Linux only. macOS uses native traffic
              lights on the left (titleBarStyle: Overlay), so we omit our custom
              minimize/maximize/close here and keep just Settings. */}
          {!IS_MACOS && (
            <>
              <button
                onClick={handleMinimize}
                className="h-full px-4 flex items-center justify-center text-cyber-text-secondary hover:bg-cyber-text/20 hover:text-cyber-text transition-colors"
              >
                <Minus size={14} />
              </button>
              <button
                onClick={handleMaximize}
                className="h-full px-4 flex items-center justify-center text-cyber-text-secondary hover:bg-cyber-text/20 hover:text-cyber-text transition-colors"
              >
                {isMaximized ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
              </button>
              <button
                onClick={handleClose}
                className="h-full px-4 flex items-center justify-center text-cyber-text-secondary hover:bg-red-500/20 hover:text-red-400 transition-colors"
              >
                <X size={14} />
              </button>
            </>
          )}
        </div>
      </div>

      {/* Close Window Dialog */}
      <CloseWindowDialog isOpen={showCloseDialog} onClose={handleCloseDialogChoice} />
    </>
  );
};
