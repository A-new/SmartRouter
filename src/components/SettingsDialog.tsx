// SettingsDialog — Global settings modal (gear button in title bar)
import React, { useState, useEffect, useCallback, useRef } from 'react';
import { X, Globe, ExternalLink, Sun, Moon, Monitor, Power } from 'lucide-react';
import { getVersion } from '@tauri-apps/api/app';
import { enable, disable, isEnabled } from '@tauri-apps/plugin-autostart';
import { MiniSelect } from './MiniSelect';
import { useI18n } from '../hooks/useI18n';
import * as api from '../api/tauri';
import { useThemeStore, type ThemeMode } from '../stores/themeStore';

// All supported locales
const LOCALE_OPTIONS = [
  { id: 'en', label: 'English' },
  { id: 'zh-Hans', label: '简体中文' },
  { id: 'zh-Hant', label: '繁體中文' },
  { id: 'ja', label: '日本語' },
];

interface SettingsDialogProps {
  isOpen: boolean;
  onClose: () => void;
  locale: string;
  onLocaleChange: (locale: string) => void;
}

export const SettingsDialog: React.FC<SettingsDialogProps> = ({
  isOpen,
  onClose,
  locale,
  onLocaleChange,
}) => {
  const { t } = useI18n();
  const [isAnimatingOut, setIsAnimatingOut] = useState(false);
  const [appVersion, setAppVersion] = useState<string>('');
  const [closeToTray, setCloseToTray] = useState<boolean | null>(false);
  // Launch-at-startup toggle. Source of truth is the autostart plugin's OS
  // registration (isEnabled), queried each time the dialog opens, so changes
  // made outside the app (msconfig, Login Items) are reflected. Default off.
  const [launchAtStartup, setLaunchAtStartup] = useState(false);
  const themeMode = useThemeStore((s) => s.mode);
  const setThemeMode = useThemeStore((s) => s.setMode);
  const dialogRef = useRef<HTMLDivElement>(null);

  // Read the installed binary version from Tauri at runtime — single source of truth (tauri.conf.json).
  useEffect(() => {
    getVersion()
      .then(setAppVersion)
      .catch(() => setAppVersion(''));
  }, []);

  // Load backend settings each time the dialog opens.
  useEffect(() => {
    if (isOpen) {
      // Reflect the real OS autostart registration (may have been toggled
      // outside the app). Default to false if the query fails.
      isEnabled()
        .then(setLaunchAtStartup)
        .catch(() => {});
      api.getSettings().then((settings) => {
        // null ("always ask") is a real selectable value, not a missing
        // field — the backend stores it as Option<bool>=None. When the user
        // explicitly chose it (closeWindowBehaviorSet === true), preserve
        // null so the dialog reflects their choice instead of resetting to
        // "quit directly" (false). Only fall back to false for the genuinely
        // unset case (never configured), matching the original default.
        setCloseToTray(
          settings.closeWindowBehaviorSet && settings.closeToTray === null
            ? null
            : (settings.closeToTray ?? false)
        );
      });
    }
  }, [isOpen]);

  // Toggle launch-at-startup: register/unregister the OS autostart entry.
  // The entry launches with --minimized so a boot start stays in the tray.
  const handleLaunchAtStartupChange = useCallback((value: boolean) => {
    setLaunchAtStartup(value);
    if (value) {
      enable().catch(() => setLaunchAtStartup(false));
    } else {
      disable().catch(() => setLaunchAtStartup(true));
    }
  }, []);

  // Save closeToTray setting when it changes
  const handleCloseToTrayChange = useCallback(async (value: boolean | null) => {
    setCloseToTray(value);
    const settings = await api.getSettings();
    // Mark the close behavior as explicitly chosen. This both suppresses the
    // first-time onboarding dialog and — critically — prevents that dialog from
    // later overwriting an explicit "always ask" (null) selection.
    await api.saveSettings({
      ...settings,
      closeToTray: value,
      closeWindowBehaviorSet: true,
    });
  }, []);

  // Close with animation
  const handleClose = useCallback(() => {
    setIsAnimatingOut(true);
    setTimeout(() => {
      setIsAnimatingOut(false);
      onClose();
    }, 200);
  }, [onClose]);

  // ESC to close
  useEffect(() => {
    if (!isOpen) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') handleClose();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [isOpen, handleClose]);

  if (!isOpen) return null;

  return (
    <div
      className={`fixed inset-0 z-[9998] flex items-center justify-center transition-all duration-200 ${
        isAnimatingOut ? 'opacity-0' : 'opacity-100'
      }`}
    >
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={handleClose} />

      {/* Dialog */}
      <div
        ref={dialogRef}
        className={`relative w-[440px] max-w-[92vw] border border-cyber-border/30 bg-cyber-surface shadow-2xl rounded-xl overflow-hidden transition-all duration-200 ${
          isAnimatingOut ? 'scale-95 opacity-0' : 'scale-100 opacity-100'
        }`}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Top accent line */}
        <div className="h-px w-full bg-cyber-border" />

        {/* Header */}
        <div className="px-6 pt-5 pb-4 flex items-center justify-between">
          <span className="text-lg font-bold text-cyber-text">{t('settings.title')}</span>
          <button
            onClick={handleClose}
            className="text-cyber-text-secondary hover:text-cyber-text transition-colors"
          >
            <X size={18} />
          </button>
        </div>

        {/* Content */}
        <div className="px-6 pb-6 space-y-5">
          {/* Version */}
          <div className="flex items-center justify-between">
            <span className="text-[14px] text-cyber-text-secondary">{t('settings.version')}</span>
            <span className="text-[14px] font-mono font-medium text-cyber-text">
              {appVersion ? `v${appVersion}` : '—'}
            </span>
          </div>

          {/* Divider */}
          <div className="h-px bg-cyber-border/50" />

          {/* Appearance — Light / Dark / System */}
          <div className="space-y-2.5">
            <div className="flex items-center gap-2">
              <Sun size={14} className="text-cyber-text-secondary" />
              <span className="text-[14px] font-medium text-cyber-text-secondary">
                {t('settings.appearance')}
              </span>
            </div>
            <ThemeSegmented
              value={themeMode}
              onChange={setThemeMode}
              labels={{
                light: t('settings.themeLight'),
                dark: t('settings.themeDark'),
                system: t('settings.themeSystem'),
              }}
            />
          </div>

          {/* Divider */}
          <div className="h-px bg-cyber-border/50" />

          {/* Close Window Behavior */}
          <div className="space-y-2.5">
            <div className="flex items-center gap-2">
              <X size={14} className="text-cyber-text-secondary" />
              <span className="text-[14px] font-medium text-cyber-text-secondary">
                {t('settings.closeWindowBehavior')}
              </span>
            </div>
            <div className="flex gap-1 p-1 bg-cyber-input border border-cyber-border rounded-button">
              <button
                onClick={() => handleCloseToTrayChange(false)}
                className={`flex-1 h-9 flex items-center justify-center text-[13px] transition-colors rounded ${
                  closeToTray === false
                    ? 'bg-cyber-elevated text-cyber-text font-semibold'
                    : 'text-cyber-text-secondary hover:text-cyber-text hover:bg-cyber-elevated'
                }`}
              >
                {t('settings.closeDirectly')}
              </button>
              <button
                onClick={() => handleCloseToTrayChange(true)}
                className={`flex-1 h-9 flex items-center justify-center text-[13px] transition-colors rounded ${
                  closeToTray === true
                    ? 'bg-cyber-elevated text-cyber-text font-semibold'
                    : 'text-cyber-text-secondary hover:text-cyber-text hover:bg-cyber-elevated'
                }`}
              >
                {t('settings.closeToTray')}
              </button>
              <button
                onClick={() => handleCloseToTrayChange(null)}
                className={`flex-1 h-9 flex items-center justify-center text-[13px] transition-colors rounded ${
                  closeToTray === null
                    ? 'bg-cyber-elevated text-cyber-text font-semibold'
                    : 'text-cyber-text-secondary hover:text-cyber-text hover:bg-cyber-elevated'
                }`}
              >
                {t('settings.alwaysAsk')}
              </button>
            </div>
          </div>

          {/* Divider */}
          <div className="h-px bg-cyber-border/50" />

          {/* Language */}
          <div className="space-y-2.5">
            <div className="flex items-center gap-2">
              <Globe size={14} className="text-cyber-text-secondary" />
              <span className="text-[14px] font-medium text-cyber-text-secondary">
                {t('settings.language')}
              </span>
            </div>
            <MiniSelect value={locale} onChange={onLocaleChange} options={LOCALE_OPTIONS} />
          </div>

          {/* Divider */}
          <div className="h-px bg-cyber-border/50" />

          {/* Launch at startup - boots hidden in the tray (default off) */}
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Power size={14} className="text-cyber-text-secondary" />
              <span className="text-[14px] font-medium text-cyber-text-secondary">
                {t('settings.launchAtStartup')}
              </span>
            </div>
            <ToggleSwitch checked={launchAtStartup} onChange={handleLaunchAtStartupChange} />
          </div>

          {/* Divider */}
          <div className="h-px bg-cyber-border/50" />

          {/* Mod credit */}
          <div className="pt-2 flex justify-center">
            <button
              onClick={() => api.openExternal('https://www.52pojie.cn/thread-2127358-1-1.html')}
              className="text-[14px] font-mono font-medium text-cyber-text-secondary hover:text-cyber-text transition-colors flex items-center gap-1.5"
            >
              Mod by A-new <ExternalLink size={13} />
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};

// Compact on/off switch.
const ToggleSwitch: React.FC<{ checked: boolean; onChange: (v: boolean) => void }> = ({
  checked,
  onChange,
}) => (
  <button
    type="button"
    role="switch"
    aria-checked={checked}
    onClick={() => onChange(!checked)}
    className={`relative inline-flex h-5 w-9 flex-shrink-0 items-center rounded-full outline-none transition-colors ${
      checked ? 'bg-cyber-accent' : 'bg-cyber-border'
    }`}
  >
    <span
      className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white transition-transform duration-200 ${
        checked ? 'translate-x-[18px]' : 'translate-x-1'
      }`}
    />
  </button>
);

// 3-button segmented control for the theme: Light / Dark / System.
const ThemeSegmented: React.FC<{
  value: ThemeMode;
  onChange: (mode: ThemeMode) => void;
  labels: { light: string; dark: string; system: string };
}> = ({ value, onChange, labels }) => {
  const opts: Array<{ id: ThemeMode; icon: React.ReactNode; label: string }> = [
    { id: 'light', icon: <Sun size={14} />, label: labels.light },
    { id: 'dark', icon: <Moon size={14} />, label: labels.dark },
    { id: 'system', icon: <Monitor size={14} />, label: labels.system },
  ];
  return (
    <div className="flex gap-1 p-1 bg-cyber-input border border-cyber-border rounded-button">
      {opts.map((o) => {
        const active = value === o.id;
        return (
          <button
            key={o.id}
            onClick={() => onChange(o.id)}
            className={`flex-1 h-9 flex items-center justify-center gap-1.5 text-[14px] transition-colors rounded ${
              active
                ? 'bg-cyber-elevated text-cyber-text font-semibold'
                : 'text-cyber-text-secondary hover:text-cyber-text hover:bg-cyber-elevated'
            }`}
          >
            {o.icon}
            {o.label}
          </button>
        );
      })}
    </div>
  );
};
