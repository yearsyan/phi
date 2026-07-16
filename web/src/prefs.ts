import { DEFAULT_LOCALE, LOCALES, type Locale } from './i18n/translations.ts';

/**
 * Persisted UI preferences: color theme and display locale.
 *
 * Theme defaults to the OS preference on first load; locale defaults to English
 * (the daemon protocol and most shared terms stay in English regardless).
 */

export type Theme = 'dark' | 'light';

const KEY_THEME = 'phi.prefs.theme';
const KEY_LOCALE = 'phi.prefs.locale';

function detectPreferredTheme(): Theme {
  if (typeof window === 'undefined' || !window.matchMedia) return 'dark';
  return window.matchMedia('(prefers-color-scheme: light)').matches
    ? 'light'
    : 'dark';
}

export function readTheme(): Theme {
  if (typeof localStorage === 'undefined') return detectPreferredTheme();
  const stored = localStorage.getItem(KEY_THEME);
  if (stored === 'dark' || stored === 'light') return stored;
  return detectPreferredTheme();
}

export function writeTheme(theme: Theme): void {
  if (typeof localStorage === 'undefined') return;
  localStorage.setItem(KEY_THEME, theme);
}

export function readLocale(): Locale {
  if (typeof localStorage === 'undefined') return DEFAULT_LOCALE;
  const stored = localStorage.getItem(KEY_LOCALE);
  if (stored && (LOCALES as readonly string[]).includes(stored)) {
    return stored as Locale;
  }
  return DEFAULT_LOCALE;
}

export function writeLocale(locale: Locale): void {
  if (typeof localStorage === 'undefined') return;
  localStorage.setItem(KEY_LOCALE, locale);
}
