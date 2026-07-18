/** @vitest-environment jsdom */

import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { I18nProvider } from '../../../i18n/I18nProvider.tsx';
import type { ToolCall } from '../../../types/wire.ts';
import { ToolCallItem } from './ToolCallItem.tsx';

function renderItem(
  props: Partial<Parameters<typeof ToolCallItem>[0]> & { call: ToolCall },
) {
  return render(
    <I18nProvider initialLocale="en">
      <ToolCallItem
        status="done"
        progress={[]}
        output={null}
        streamingArgs={null}
        {...props}
      />
    </I18nProvider>,
  );
}

describe('ToolCallItem', () => {
  afterEach(cleanup);

  it('starts collapsed while running and reveals streamed arguments on demand', () => {
    renderItem({
      call: { id: 'c1', name: 'shell', arguments: null },
      status: 'running',
      streamingArgs: '{"command":"ls',
    });

    const trigger = screen.getByRole('button', { name: /shell/ });
    expect(trigger.getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryByText(/\{"command":"ls/)).toBeNull();

    fireEvent.click(trigger);
    expect(trigger.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByText(/\{"command":"ls/)).toBeTruthy();
  });

  it('starts collapsed when done and reveals output after expanding', () => {
    renderItem({
      call: { id: 'c1', name: 'read', arguments: { path: '/tmp/a.ts' } },
      output: 'file body',
    });

    const trigger = screen.getByRole('button', { name: /read/ });
    expect(trigger.getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryByText('file body')).toBeNull();

    fireEvent.click(trigger);
    expect(screen.getByText('file body')).toBeTruthy();
    // Arguments render as key-value rows.
    expect(screen.getByText('path')).toBeTruthy();
    expect(screen.getByText('/tmp/a.ts')).toBeTruthy();
  });

  it('marks failed tools and keeps the summary visible when collapsed', () => {
    renderItem({
      call: { id: 'c1', name: 'shell', arguments: { command: 'make test' } },
      status: 'error',
      output: 'exit 1',
    });

    expect(screen.getByText('(make test)')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /shell/ }));
    expect(screen.getByText('Failed')).toBeTruthy();
    expect(screen.getByText('exit 1')).toBeTruthy();
  });

  it('is not expandable when there is nothing to show', () => {
    renderItem({ call: { id: 'c1', name: 'noop', arguments: null } });

    const trigger = screen.getByRole('button', { name: /noop/ });
    expect(trigger.getAttribute('aria-expanded')).toBeNull();
  });
});
