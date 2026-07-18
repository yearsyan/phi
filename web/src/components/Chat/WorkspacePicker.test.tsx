/** @vitest-environment jsdom */

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../i18n/I18nProvider.tsx';
import { WorkspacePicker } from './WorkspacePicker.tsx';

const apiMocks = vi.hoisted(() => ({
  browseWorkspace: vi.fn(),
}));

vi.mock('../../api/http.ts', () => apiMocks);

describe('WorkspacePicker', () => {
  beforeEach(() => {
    installLocalStorage();
    apiMocks.browseWorkspace.mockReset();
    localStorage.clear();
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('opens a bounded recent-workspace menu before the browser modal', () => {
    const recent = Array.from(
      { length: 16 },
      (_, index) => `/workspace/project-${index}`,
    );
    localStorage.setItem('phi.prefs.recentWorkspaces', JSON.stringify(recent));

    renderPicker('/workspace/current');
    fireEvent.click(screen.getByRole('button', { name: 'current' }));

    expect(
      screen.getByRole('menu', { name: 'Recent workspaces' }),
    ).toBeTruthy();
    const list = screen.getByTestId('workspace-scroll-list');
    expect(list.className).toContain('workspaceList');
    expect(screen.getAllByRole('menuitemradio')).toHaveLength(12);
    expect(apiMocks.browseWorkspace).not.toHaveBeenCalled();
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('selects an existing recent workspace without opening the browser', () => {
    localStorage.setItem(
      'phi.prefs.recentWorkspaces',
      JSON.stringify(['/workspace/other']),
    );
    const onSelect = vi.fn();
    renderPicker('/workspace/current', onSelect);

    fireEvent.click(screen.getByRole('button', { name: 'current' }));
    fireEvent.click(
      screen.getByRole('menuitemradio', { name: /other.*\/workspace\/other/ }),
    );

    expect(onSelect).toHaveBeenCalledWith('/workspace/other');
    expect(apiMocks.browseWorkspace).not.toHaveBeenCalled();
  });

  it('opens Add workspace as a modal, navigates, and returns the selection', async () => {
    apiMocks.browseWorkspace
      .mockResolvedValueOnce({
        path: '/workspace',
        parent: '/',
        directories: [{ name: 'Project A', path: '/workspace/Project A' }],
        truncated: false,
      })
      .mockResolvedValueOnce({
        path: '/workspace/Project A',
        parent: '/workspace',
        directories: [],
        truncated: false,
      });
    const onSelect = vi.fn();
    renderPicker('/workspace', onSelect);

    fireEvent.click(screen.getByRole('button', { name: 'workspace' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Add workspace' }));

    expect(screen.getByRole('dialog', { name: 'Add workspace' })).toBeTruthy();
    await waitFor(() => {
      expect(apiMocks.browseWorkspace).toHaveBeenCalledWith(
        'daemon-key',
        '/workspace',
      );
    });

    fireEvent.click(screen.getByRole('button', { name: /Project A/ }));
    await waitFor(() => {
      expect(apiMocks.browseWorkspace).toHaveBeenLastCalledWith(
        'daemon-key',
        '/workspace/Project A',
      );
    });

    fireEvent.click(screen.getByRole('button', { name: 'Use this folder' }));
    expect(onSelect).toHaveBeenCalledWith('/workspace/Project A');
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('closes the Add workspace modal without changing the selection', async () => {
    apiMocks.browseWorkspace.mockResolvedValue({
      path: '/workspace',
      parent: '/',
      directories: [],
      truncated: false,
    });
    const onSelect = vi.fn();
    renderPicker('/workspace', onSelect);

    fireEvent.click(screen.getByRole('button', { name: 'workspace' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Add workspace' }));
    await waitFor(() => expect(apiMocks.browseWorkspace).toHaveBeenCalled());
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(screen.queryByRole('dialog')).toBeNull();
    expect(onSelect).not.toHaveBeenCalled();
  });
});

function renderPicker(workspace: string, onSelect = vi.fn()) {
  return render(
    <I18nProvider initialLocale="en">
      <WorkspacePicker
        authKey="daemon-key"
        workspace={workspace}
        onSelect={onSelect}
      />
    </I18nProvider>,
  );
}

function installLocalStorage() {
  const values = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    get length() {
      return values.size;
    },
    clear: vi.fn(() => values.clear()),
    getItem: vi.fn((key: string) => values.get(key) ?? null),
    key: vi.fn((index: number) => Array.from(values.keys())[index] ?? null),
    removeItem: vi.fn((key: string) => values.delete(key)),
    setItem: vi.fn((key: string, value: string) => {
      values.set(key, String(value));
    }),
  });
}
