/** @vitest-environment jsdom */

import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../i18n/I18nProvider.tsx';
import type {
  PublicProviderConfig,
  SkillSummary,
  Usage,
} from '../../types/wire.ts';
import { Composer } from './Composer.tsx';

const reviewSkill: SkillSummary = {
  name: 'review',
  display_name: 'Code review',
  description: 'Review the current change for correctness.',
  argument_hint: '[focus]',
  model_invocable: true,
  user_invocable: true,
};

const usage: Usage = {
  last: {
    input_tokens: 80,
    output_tokens: 20,
    total_tokens: 450,
    cached_input_tokens: 40,
  },
  context: {
    max_tokens: 1_000,
    used_tokens: 100,
    remaining_tokens: 900,
  },
  cumulative: {
    input_tokens: 200,
    output_tokens: 40,
    total_tokens: 700,
    cached_input_tokens: 100,
  },
};

const providerProfiles: PublicProviderConfig[] = [
  {
    profile_id: 'default',
    provider: 'openai_chat',
    api_key_configured: true,
    base_url: 'https://openai.example.test/v1',
    model: 'test-model',
    max_output_tokens: 4096,
    max_context_tokens: 128000,
    temperature: null,
    reasoning_effort: null,
    max_retries: 10,
    request_timeout_secs: 30,
    stream_idle_timeout_secs: 120,
    revision: 1,
  },
  {
    profile_id: 'anthropic-prod',
    provider: 'anthropic',
    api_key_configured: true,
    base_url: 'https://anthropic.example.test',
    model: 'claude-test',
    max_output_tokens: 4096,
    max_context_tokens: 200000,
    temperature: null,
    reasoning_effort: null,
    max_retries: 10,
    request_timeout_secs: 30,
    stream_idle_timeout_secs: 120,
    revision: 2,
  },
];

function renderComposer(
  overrides: Partial<ComponentProps<typeof Composer>> = {},
) {
  const props: ComponentProps<typeof Composer> = {
    disabled: false,
    busy: false,
    canStop: false,
    canConfigure: true,
    sessionActivated: true,
    canCompact: true,
    queuedCount: 0,
    capabilityMode: 'full_access',
    profileId: 'default',
    providerProfiles,
    model: 'test-model',
    reasoningEffort: null,
    usage,
    skills: [reviewSkill],
    onSend: vi.fn(() => true),
    onStop: vi.fn(),
    onSetCapabilityMode: vi.fn(),
    onSelectProvider: vi.fn(),
    onSetModel: vi.fn(),
    onSetReasoningEffort: vi.fn(),
    onCompact: vi.fn(() => true),
    ...overrides,
  };
  return {
    props,
    ...render(
      <I18nProvider initialLocale="en">
        <Composer {...props} />
      </I18nProvider>,
    ),
  };
}

describe('Composer', () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('filters the slash palette to commands and user-invocable skills', () => {
    renderComposer({
      skills: [
        reviewSkill,
        {
          name: 'hidden',
          description: 'Model-only skill',
          model_invocable: true,
          user_invocable: false,
        },
      ],
    });

    fireEvent.change(screen.getByLabelText('Message Phi'), {
      target: { value: '/' },
    });

    expect(screen.getByText('/compact')).toBeTruthy();
    expect(screen.getByText('/review')).toBeTruthy();
    expect(screen.queryByText('/hidden')).toBeNull();
  });

  it('invokes a selected skill with the remaining slash arguments', () => {
    const { props } = renderComposer();
    const textarea = screen.getByLabelText('Message Phi');

    fireEvent.change(textarea, { target: { value: '/rev' } });
    fireEvent.click(screen.getByRole('option', { name: /\/review/ }));
    expect((textarea as HTMLTextAreaElement).value).toBe('/review ');

    fireEvent.change(textarea, {
      target: { value: '/review security' },
    });
    fireEvent.keyDown(textarea, { key: 'Enter' });

    expect(props.onSend).toHaveBeenCalledWith('security', {
      name: 'review',
    });
    expect((textarea as HTMLTextAreaElement).value).toBe('');
  });

  it('routes the compact slash command without sending a model prompt', () => {
    const { props } = renderComposer();
    const textarea = screen.getByLabelText('Message Phi');

    fireEvent.change(textarea, {
      target: { value: '/compact preserve decisions' },
    });
    fireEvent.keyDown(textarea, { key: 'Enter' });

    expect(props.onCompact).toHaveBeenCalledWith('preserve decisions');
    expect(props.onSend).not.toHaveBeenCalled();
  });

  it('moves context, model, reasoning, and capability controls into the composer', () => {
    const { props } = renderComposer();

    const contextButton = screen.getByRole('button', {
      name: 'Context capacity',
    });
    expect(screen.getByText('10.0%')).toBeTruthy();
    expect(screen.queryByRole('tooltip')).toBeNull();

    fireEvent.mouseEnter(contextButton);
    expect(screen.getByRole('tooltip')).toBeTruthy();
    expect(screen.getByText('100 / 1K (10.0%)')).toBeTruthy();
    expect(screen.getByText('50%')).toBeTruthy();

    fireEvent.mouseLeave(contextButton);
    expect(screen.queryByRole('tooltip')).toBeNull();

    fireEvent.change(screen.getByLabelText('Access'), {
      target: { value: 'read_only' },
    });
    expect(props.onSetCapabilityMode).toHaveBeenCalledWith('read_only');

    fireEvent.change(screen.getByLabelText('Reasoning effort'), {
      target: { value: 'high' },
    });
    expect(props.onSetReasoningEffort).toHaveBeenCalledWith('high');

    fireEvent.click(screen.getByRole('button', { name: /test-model/ }));
    fireEvent.change(screen.getByLabelText('Session model'), {
      target: { value: 'next-model' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
    expect(props.onSetModel).toHaveBeenCalledWith('next-model');
  });

  it('uses a profile model for the next request in an activated conversation', () => {
    const confirm = vi.spyOn(window, 'confirm');
    const { props } = renderComposer();
    const textarea = screen.getByLabelText('Message Phi');

    fireEvent.change(textarea, { target: { value: 'keep this draft' } });
    fireEvent.click(screen.getByRole('button', { name: /test-model/ }));
    expect(
      screen.getByText(
        'This conversation keeps its Provider connection. Choosing a profile changes only the model used by the next request.',
      ),
    ).toBeTruthy();
    fireEvent.click(
      screen.getByRole('option', { name: /anthropic-prod.*claude-test/ }),
    );

    expect(props.onSetModel).toHaveBeenCalledWith('claude-test');
    expect(props.onSelectProvider).not.toHaveBeenCalled();
    expect(confirm).not.toHaveBeenCalled();
    expect((textarea as HTMLTextAreaElement).value).toBe('keep this draft');
  });

  it('switches the Provider connection before a new conversation is activated', () => {
    const { props } = renderComposer({ sessionActivated: false });

    fireEvent.click(screen.getByRole('button', { name: /test-model/ }));
    expect(
      screen.getByText(
        'Before the first message, choosing a profile changes the Provider connection for this new chat.',
      ),
    ).toBeTruthy();
    fireEvent.click(
      screen.getByRole('option', { name: /anthropic-prod.*claude-test/ }),
    );

    expect(props.onSelectProvider).toHaveBeenCalledWith('anthropic-prod');
    expect(props.onSetModel).not.toHaveBeenCalled();
  });

  it('keeps an unsent draft when Provider switching is cancelled', () => {
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
    const { props } = renderComposer({ sessionActivated: false });
    const textarea = screen.getByLabelText('Message Phi');

    fireEvent.change(textarea, { target: { value: 'unsent work' } });
    fireEvent.click(screen.getByRole('button', { name: /test-model/ }));
    fireEvent.click(
      screen.getByRole('option', { name: /anthropic-prod.*claude-test/ }),
    );

    expect(confirm).toHaveBeenCalledOnce();
    expect(props.onSelectProvider).not.toHaveBeenCalled();
    expect((textarea as HTMLTextAreaElement).value).toBe('unsent work');
  });
});
