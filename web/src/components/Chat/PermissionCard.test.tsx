/** @vitest-environment jsdom */

import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../i18n/I18nProvider.tsx';
import type { ToolPermissionPrompt } from '../../types/wire.ts';
import { PermissionCard } from './PermissionCard.tsx';

const request: ToolPermissionPrompt = {
  permission_id: 'permission-1',
  call: {
    id: 'call-1',
    name: 'bash',
    arguments: { command: 'ls -la' },
  },
  effect: 'external_side_effect',
  capability_mode: 'workspace_edit',
  suggestions: [
    { tool_name: 'bash', pattern: 'ls *' },
    { tool_name: 'bash', pattern: 'ls -la' },
  ],
};

describe('PermissionCard', () => {
  afterEach(cleanup);

  it('shows the command and submits only a server-suggested remembered rule', () => {
    const onDecision = vi.fn(() => true);
    render(
      <I18nProvider initialLocale="en">
        <PermissionCard request={request} onDecision={onDecision} />
      </I18nProvider>,
    );

    expect(screen.getByText('ls -la')).toBeTruthy();
    fireEvent.change(screen.getByLabelText('Remembered rule'), {
      target: { value: '1' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Allow for session' }));

    expect(onDecision).toHaveBeenCalledWith('permission-1', {
      type: 'allow_for_session',
      rule: { tool_name: 'bash', pattern: 'ls -la' },
    });
  });

  it('supports one-shot approval and denial without inventing a rule', () => {
    const onDecision = vi.fn(() => false);
    render(
      <I18nProvider initialLocale="en">
        <PermissionCard request={request} onDecision={onDecision} />
      </I18nProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Allow once' }));
    fireEvent.click(screen.getByRole('button', { name: 'Deny' }));

    expect(onDecision).toHaveBeenNthCalledWith(1, 'permission-1', {
      type: 'allow_once',
    });
    expect(onDecision).toHaveBeenNthCalledWith(2, 'permission-1', {
      type: 'deny',
    });
  });
});
