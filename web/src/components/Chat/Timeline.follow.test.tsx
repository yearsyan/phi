/** @vitest-environment jsdom */

import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../i18n/I18nProvider.tsx';
import type { TimelineItem } from '../../state/timeline.ts';
import { Timeline } from './Timeline.tsx';

const virtuosoMocks = vi.hoisted(() => ({
  autoscrollToBottom: vi.fn(),
  scrollTo: vi.fn(),
  scrollToIndex: vi.fn(),
  props: null as Record<string, unknown> | null,
}));

vi.mock('react-virtuoso', async () => {
  const React = await import('react');

  return {
    Virtuoso: React.forwardRef<unknown, Record<string, unknown>>(
      function MockVirtuoso(props, ref) {
        const scrollerRef = React.useRef<HTMLDivElement>(null);
        const provideScroller = props.scrollerRef as
          | ((scroller: HTMLElement | null) => void)
          | undefined;

        virtuosoMocks.props = props;
        React.useImperativeHandle(
          ref,
          () => ({
            autoscrollToBottom: virtuosoMocks.autoscrollToBottom,
            scrollTo: virtuosoMocks.scrollTo,
            scrollToIndex: virtuosoMocks.scrollToIndex,
          }),
          [],
        );
        React.useLayoutEffect(() => {
          provideScroller?.(scrollerRef.current);
          return () => provideScroller?.(null);
        }, [provideScroller]);

        return React.createElement('div', {
          ref: scrollerRef,
          'data-testid': 'virtuoso-scroller',
        });
      },
    ),
  };
});

interface RelevantVirtuosoProps {
  atBottomStateChange?: (isAtBottom: boolean) => void;
  followOutput?: (isAtBottom: boolean) => 'auto' | false;
  totalListHeightChanged?: (height: number) => void;
}

describe('Timeline output following', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    virtuosoMocks.props = null;
    vi.stubGlobal(
      'requestAnimationFrame',
      (callback: FrameRequestCallback) =>
        setTimeout(() => callback(0), 0) as unknown as number,
    );
    vi.stubGlobal('cancelAnimationFrame', (frame: number) => {
      clearTimeout(frame);
    });
  });

  afterEach(() => {
    cleanup();
    vi.clearAllTimers();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('keeps following when streaming content temporarily pushes the viewport off the bottom', () => {
    const { rerender } = renderTimeline(assistantItem('first line'));
    flushAnimationFrame();
    virtuosoMocks.autoscrollToBottom.mockClear();

    act(() => latestVirtuosoProps().atBottomStateChange?.(false));
    rerender(timeline(assistantItem('first line\nsecond line')));
    act(() => latestVirtuosoProps().totalListHeightChanged?.(360));
    flushAnimationFrame();

    expect(virtuosoMocks.autoscrollToBottom).toHaveBeenCalledTimes(1);
    expect(latestVirtuosoProps().followOutput?.(false)).toBe('auto');
    expect(screen.queryByRole('button', { name: 'Latest' })).toBeNull();
  });

  it('pauses after an explicit upward scroll and resumes once the user reaches the bottom', () => {
    const { rerender } = renderTimeline(assistantItem('first line'));
    flushAnimationFrame();
    virtuosoMocks.autoscrollToBottom.mockClear();

    fireEvent.wheel(screen.getByTestId('virtuoso-scroller'), { deltaY: -24 });
    expect(latestVirtuosoProps().followOutput?.(true)).toBe(false);
    expect(screen.getByRole('button', { name: 'Latest' })).toBeTruthy();

    rerender(timeline(assistantItem('first line\nsecond line')));
    act(() => latestVirtuosoProps().totalListHeightChanged?.(360));
    flushAnimationFrame();
    expect(virtuosoMocks.autoscrollToBottom).not.toHaveBeenCalled();

    act(() => latestVirtuosoProps().atBottomStateChange?.(true));
    rerender(timeline(assistantItem('first line\nsecond line\nthird line')));
    act(() => latestVirtuosoProps().totalListHeightChanged?.(480));
    flushAnimationFrame();

    expect(virtuosoMocks.autoscrollToBottom).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('button', { name: 'Latest' })).toBeNull();
  });
});

function renderTimeline(item: TimelineItem) {
  return render(timeline(item));
}

function timeline(item: TimelineItem) {
  return (
    <I18nProvider initialLocale="en">
      <Timeline
        items={[item]}
        bottomInset={96}
        conversationKey="session-1"
        canFork={false}
        onFork={async () => undefined}
      />
    </I18nProvider>
  );
}

function assistantItem(text: string): TimelineItem {
  return {
    kind: 'assistant',
    key: 'assistant-draft',
    messageIndex: null,
    forkPosition: 'after',
    reasoning: '',
    text,
    streaming: true,
  };
}

function latestVirtuosoProps(): RelevantVirtuosoProps {
  if (virtuosoMocks.props === null) {
    throw new Error('Virtuoso has not rendered');
  }
  return virtuosoMocks.props as RelevantVirtuosoProps;
}

function flushAnimationFrame() {
  act(() => vi.runOnlyPendingTimers());
}
