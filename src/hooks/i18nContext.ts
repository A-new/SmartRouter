import { createContext } from 'react';
import type { TKey } from '../i18n';

export interface I18nContextValue {
  locale: string;
  setLocale: (locale: string) => void;
  t: (key: TKey) => string;
}

export const I18nContext = createContext<I18nContextValue>({
  locale: 'en',
  setLocale: () => {},
  t: (key) => key,
});
