/** @vitest-environment jsdom */

import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { I18nProvider } from '../../../i18n/I18nProvider.tsx';
import type { ToolTimelineItem } from '../../../state/timeline.ts';
import { ToolBatchItem } from './ToolBatchItem.tsx';

function tool(
  id: string,
  name: string,
  args: unknown,
  output: string,
): ToolTimelineItem {
  return {
    kind: 'tool',
    key: `tool-${id}`,
    call: { id, name, arguments: args },
    status: 'done',
    progress: [],
    output,
    streamingArgs: null,
  };
}

describe('ToolBatchItem', () => {
  afterEach(cleanup);

  it('starts as one activity summary and reveals existing tool rows on click', () => {
    render(
      <I18nProvider initialLocale="en">
        <ToolBatchItem
          tools={[
            tool('c1', 'read', { path: '/tmp/input.ts' }, 'file body'),
            tool('c2', 'bash', { command: 'npm test' }, 'all tests passed'),
          ]}
        />
      </I18nProvider>,
    );

    const batch = screen.getByRole('button', {
      name: /Reading and Executing.*2 tools/,
    });
    expect(batch.getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryByText('read')).toBeNull();
    expect(screen.queryByText('bash')).toBeNull();

    fireEvent.click(batch);
    expect(batch.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByText('read')).toBeTruthy();
    expect(screen.getByText('bash')).toBeTruthy();
    expect(screen.queryByText('/tmp/input.ts')).toBeNull();
    expect(screen.queryByText('npm test')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /read/ }));
    expect(screen.getByText('/tmp/input.ts')).toBeTruthy();
    expect(screen.getByText('file body')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: /bash/ }));
    expect(screen.getByText('npm test')).toBeTruthy();
    expect(screen.getByText('all tests passed')).toBeTruthy();
  });

  it('uses concise localized action text', () => {
    render(
      <I18nProvider initialLocale="zh">
        <ToolBatchItem
          tools={[
            tool('c1', 'edit', { path: 'a.ts' }, 'done'),
            tool('c2', 'bash', { command: 'cargo test' }, 'ok'),
          ]}
        />
      </I18nProvider>,
    );

    expect(
      screen.getByRole('button', { name: /写入和执行.*2 项/ }),
    ).toBeTruthy();
  });
});
