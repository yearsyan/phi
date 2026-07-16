/** @vitest-environment jsdom */

import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../i18n/I18nProvider.tsx';
import type { AskUserRequest } from '../../types/wire.ts';
import { AskCard } from './AskCard.tsx';

function renderCard(request: AskUserRequest, onAnswer = vi.fn(() => true)) {
  render(
    <I18nProvider initialLocale="en">
      <AskCard request={request} onAnswer={onAnswer} />
    </I18nProvider>,
  );
  return onAnswer;
}

describe('AskCard', () => {
  afterEach(cleanup);

  it('uses the daemon multiSelect field and submits multiple options', () => {
    const request: AskUserRequest = {
      ask_id: 'ask-1',
      questions: [
        {
          header: 'Scope',
          question: 'What should change?',
          multiSelect: true,
          options: [
            { label: 'UI', description: 'Update the interface' },
            { label: 'Tests', description: 'Add coverage' },
          ],
        },
      ],
    };
    const onAnswer = renderCard(request);

    fireEvent.click(screen.getByRole('button', { name: /UI/ }));
    fireEvent.click(screen.getByRole('button', { name: /Tests/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Submit answer' }));

    expect(onAnswer).toHaveBeenCalledWith('ask-1', [
      {
        question_index: 0,
        selected_options: ['UI', 'Tests'],
        custom_text: null,
      },
    ]);
  });

  it('makes a custom single-select answer mutually exclusive', () => {
    const request: AskUserRequest = {
      ask_id: 'ask-2',
      questions: [
        {
          header: 'Choice',
          question: 'Pick one',
          multiSelect: false,
          options: [
            { label: 'A', description: 'First' },
            { label: 'B', description: 'Second' },
          ],
        },
      ],
    };
    const onAnswer = renderCard(request);

    fireEvent.click(
      screen.getByText('A').closest('button') as HTMLButtonElement,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Other…' }));
    fireEvent.change(screen.getByPlaceholderText('Type a custom answer…'), {
      target: { value: 'Custom' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Submit answer' }));

    expect(onAnswer).toHaveBeenCalledWith('ask-2', [
      {
        question_index: 0,
        selected_options: [],
        custom_text: 'Custom',
      },
    ]);
  });
});
