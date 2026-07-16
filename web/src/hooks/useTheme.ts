import { useCallback, useEffect, useState } from 'react';
import { readTheme, type Theme, writeTheme } from '../prefs.ts';

const THEME_ATTRIBUTE = 'data-theme';

function applyTheme(theme: Theme) {
  if (typeof document !== 'undefined') {
    document.documentElement.setAttribute(THEME_ATTRIBUTE, theme);
  }
}

/**
 * Color-theme state. The persisted/sensed theme is applied to
 * `document.documentElement[data-theme]` immediately on mount (and whenever it
 * changes). A pre-paint script in `index.html` sets the attribute before first
 * render to avoid a flash; this hook keeps it in sync thereafter.
 */
export function useTheme(initialTheme: Theme) {
  const [theme, setThemeState] = useState<Theme>(initialTheme);

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  const setTheme = useCallback((next: Theme) => {
    setThemeState(next);
    writeTheme(next);
  }, []);

  const toggle = useCallback(() => {
    setThemeState((current) => {
      const next: Theme = current === 'dark' ? 'light' : 'dark';
      writeTheme(next);
      applyTheme(next);
      return next;
    });
  }, []);

  return { theme, setTheme, toggle };
}

/** Read the initial theme for the first render (sourced from storage/OS). */
export function initialTheme(): Theme {
  return readTheme();
}
