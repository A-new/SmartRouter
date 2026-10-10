// Self-contained strings + language picker for the crash floor (ErrorBoundary
// and main.tsx's pre-React renderBootError). Deliberately does NOT use the i18n
// system (useI18n / locale packs): the crash UI must work even when i18n is
// exactly what failed to initialize. Mirrors the app's four locales and the
// resolveLocale() mapping in src/i18n/index.ts.

export type ErrorLang = 'en' | 'zh-Hans' | 'zh-Hant' | 'ja';

interface ErrorStrings {
  title: string;
  body: string;
  copy: string;
  reload: string;
  bootTitle: string;
}

export const ERROR_MESSAGES: Record<ErrorLang, ErrorStrings> = {
  en: {
    title: 'Something went wrong',
    body: 'The app hit an unexpected error. Copy the details below, send them to us, then reload.',
    copy: 'Copy error',
    reload: 'Reload',
    bootTitle: 'Startup failed',
  },
  'zh-Hans': {
    title: '应用出错了',
    body: '界面遇到一个未处理的错误。复制下面的错误信息发给我们,然后重新加载即可。',
    copy: '复制错误信息',
    reload: '重新加载',
    bootTitle: '启动失败',
  },
  'zh-Hant': {
    title: '應用程式發生錯誤',
    body: '介面發生未處理的錯誤。請複製下方的錯誤資訊傳給我們,然後重新載入。',
    copy: '複製錯誤資訊',
    reload: '重新載入',
    bootTitle: '啟動失敗',
  },
  ja: {
    title: 'エラーが発生しました',
    body: '画面で予期しないエラーが発生しました。下のエラー情報をコピーして送信し、再読み込みしてください。',
    copy: 'エラーをコピー',
    reload: '再読み込み',
    bootTitle: '起動に失敗しました',
  },
};

/**
 * Pick the UI language WITHOUT touching the i18n system. Reads the saved
 * preference, then `<html lang>`, then `navigator.language`, and maps to one of
 * the four supported locales using the same rules as resolveLocale().
 */
export function pickErrorLang(): ErrorLang {
  let tag = '';
  try {
    tag = localStorage.getItem('echobird-locale') ?? '';
  } catch {
    /* localStorage may be unavailable */
  }
  if (!tag && typeof document !== 'undefined') {
    tag = document.documentElement.lang || '';
  }
  if (!tag && typeof navigator !== 'undefined') {
    tag = navigator.language || '';
  }
  if (/^zh[-_](TW|HK|MO|Hant)/i.test(tag)) return 'zh-Hant';
  if (/^zh/i.test(tag)) return 'zh-Hans';
  if (/^ja/i.test(tag)) return 'ja';
  return 'en';
}
