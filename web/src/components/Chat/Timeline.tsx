import {
  type ComponentPropsWithRef,
  forwardRef,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { type ListRange, Virtuoso, type VirtuosoHandle } from 'react-virtuoso';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { TimelineItem } from '../../state/timeline.ts';
import type { ForkPosition } from '../../types/wire.ts';
import { ArrowDownIcon } from '../common/Icons.tsx';
import { ConversationMap } from './ConversationMap.tsx';
import { AssistantText } from './items/AssistantText.tsx';
import { CompactionDivider } from './items/CompactionDivider.tsx';
import { StatusLine } from './items/StatusLine.tsx';
import { ToolBatchItem } from './items/ToolBatchItem.tsx';
import { ToolCallItem } from './items/ToolCallItem.tsx';
import { UserMessage } from './items/UserMessage.tsx';
import styles from './Timeline.module.css';

interface TimelineProps {
  items: TimelineItem[];
  /** Height occupied by the composer floating over the scroll viewport. */
  bottomInset: number;
  /** Changes reset the virtual list (session switch). */
  conversationKey?: string;
  canFork: boolean;
  onFork: (messageIndex: number, position: ForkPosition) => Promise<void>;
}

interface TailSpacerItem {
  kind: 'tail-spacer';
  key: 'timeline-tail-spacer';
}

type VirtualTimelineItem = TimelineItem | TailSpacerItem;

const TAIL_SPACER: TailSpacerItem = {
  kind: 'tail-spacer',
  key: 'timeline-tail-spacer',
};

export function withTimelineTailSpacer(
  items: TimelineItem[],
): VirtualTimelineItem[] {
  return [...items, TAIL_SPACER];
}

/**
 * Virtualized chat timeline. Follows the tail while the user is at the bottom
 * and stops following once they scroll up; a floating button jumps back down.
 */
export function Timeline({
  items,
  bottomInset,
  conversationKey,
  canFork,
  onFork,
}: TimelineProps) {
  const { t } = useI18n();
  const virtuosoRef = useRef<VirtuosoHandle>(null);
  const followFrameRef = useRef<number | null>(null);
  const followingTailRef = useRef(true);
  const previousBottomInsetRef = useRef(bottomInset);
  const previousConversationKeyRef = useRef(conversationKey);
  const [followingTail, setFollowingTail] = useState(true);
  const [scrollerElement, setScrollerElement] = useState<HTMLElement | null>(
    null,
  );
  const [rangeState, setRangeState] = useState<{
    conversationKey: string | undefined;
    range: ListRange;
  } | null>(null);
  const virtualItems = useMemo<VirtualTimelineItem[]>(
    () => withTimelineTailSpacer(items),
    [items],
  );
  const visibleRange =
    rangeState !== null && rangeState.conversationKey === conversationKey
      ? rangeState.range
      : null;

  const cancelScheduledFollow = useCallback(() => {
    if (followFrameRef.current !== null) {
      cancelAnimationFrame(followFrameRef.current);
      followFrameRef.current = null;
    }
  }, []);

  const pauseFollowingTail = useCallback(() => {
    followingTailRef.current = false;
    cancelScheduledFollow();
    setFollowingTail(false);
  }, [cancelScheduledFollow]);

  const resumeFollowingTail = useCallback(() => {
    followingTailRef.current = true;
    setFollowingTail(true);
  }, []);

  const scheduleFollowTail = useCallback(() => {
    if (!followingTailRef.current) {
      return;
    }

    cancelScheduledFollow();
    followFrameRef.current = requestAnimationFrame(() => {
      followFrameRef.current = null;
      if (followingTailRef.current) {
        virtuosoRef.current?.autoscrollToBottom();
      }
    });
  }, [cancelScheduledFollow]);

  const followOutput = useCallback(
    () => (followingTailRef.current ? 'auto' : false),
    [],
  );

  const jumpToBottom = useCallback(() => {
    resumeFollowingTail();
    virtuosoRef.current?.scrollTo({
      top: Number.MAX_SAFE_INTEGER,
      behavior: 'smooth',
    });
  }, [resumeFollowingTail]);

  const jumpToItem = useCallback(
    (index: number) => {
      pauseFollowingTail();
      virtuosoRef.current?.scrollToIndex({
        index,
        align: 'start',
        behavior: 'smooth',
      });
    },
    [pauseFollowingTail],
  );

  const handleRangeChanged = useCallback(
    (range: ListRange) => setRangeState({ conversationKey, range }),
    [conversationKey],
  );

  const handleAtBottomStateChange = useCallback(
    (isAtBottom: boolean) => {
      if (isAtBottom) {
        resumeFollowingTail();
      }
    },
    [resumeFollowingTail],
  );

  const handleScrollerRef = useCallback(
    (scroller: HTMLElement | Window | null) => {
      setScrollerElement(scroller instanceof HTMLElement ? scroller : null);
    },
    [],
  );

  useEffect(() => {
    if (scrollerElement === null) {
      return;
    }

    let pointerActive = false;
    let previousScrollTop = scrollerElement.scrollTop;

    const handleWheel = (event: WheelEvent) => {
      if (event.deltaY < 0) {
        pauseFollowingTail();
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (isUpwardScrollKey(event)) {
        pauseFollowingTail();
      }
    };
    const handlePointerDown = () => {
      pointerActive = true;
      previousScrollTop = scrollerElement.scrollTop;
    };
    const handleScroll = () => {
      const nextScrollTop = scrollerElement.scrollTop;
      if (pointerActive && nextScrollTop < previousScrollTop - 1) {
        pauseFollowingTail();
      }
      previousScrollTop = nextScrollTop;
    };
    const handlePointerEnd = () => {
      pointerActive = false;
    };

    scrollerElement.addEventListener('wheel', handleWheel, { passive: true });
    scrollerElement.addEventListener('keydown', handleKeyDown);
    scrollerElement.addEventListener('pointerdown', handlePointerDown);
    scrollerElement.addEventListener('scroll', handleScroll, { passive: true });
    window.addEventListener('pointerup', handlePointerEnd);
    window.addEventListener('pointercancel', handlePointerEnd);

    return () => {
      scrollerElement.removeEventListener('wheel', handleWheel);
      scrollerElement.removeEventListener('keydown', handleKeyDown);
      scrollerElement.removeEventListener('pointerdown', handlePointerDown);
      scrollerElement.removeEventListener('scroll', handleScroll);
      window.removeEventListener('pointerup', handlePointerEnd);
      window.removeEventListener('pointercancel', handlePointerEnd);
    };
  }, [pauseFollowingTail, scrollerElement]);

  useLayoutEffect(() => {
    if (previousConversationKeyRef.current !== conversationKey) {
      previousConversationKeyRef.current = conversationKey;
      resumeFollowingTail();
    }
  }, [conversationKey, resumeFollowingTail]);

  useLayoutEffect(() => {
    if (previousBottomInsetRef.current !== bottomInset) {
      previousBottomInsetRef.current = bottomInset;
      scheduleFollowTail();
    }
  }, [bottomInset, scheduleFollowTail]);

  useEffect(() => cancelScheduledFollow, [cancelScheduledFollow]);

  return (
    <div className={styles.viewport}>
      <ConversationMap
        items={items}
        visibleRange={visibleRange}
        onJump={jumpToItem}
      />
      <Virtuoso
        key={conversationKey}
        ref={virtuosoRef}
        data={virtualItems}
        className={styles.scroller}
        scrollerRef={handleScrollerRef}
        followOutput={followOutput}
        atBottomStateChange={handleAtBottomStateChange}
        totalListHeightChanged={scheduleFollowTail}
        rangeChanged={handleRangeChanged}
        atBottomThreshold={80}
        defaultItemHeight={120}
        increaseViewportBy={{ top: 400, bottom: 400 }}
        initialTopMostItemIndex={{
          index: virtualItems.length - 1,
          align: 'end',
        }}
        computeItemKey={(_index, item) => item.key}
        components={{ List }}
        itemContent={(index, item) =>
          item.kind === 'tail-spacer' ? (
            <BottomSpacer />
          ) : (
            <div className={rowClass(items, index)}>
              <TimelineRow item={item} canFork={canFork} onFork={onFork} />
            </div>
          )
        }
      />
      {!followingTail && items.length > 0 && (
        <button
          type="button"
          className={styles.jumpButton}
          onClick={jumpToBottom}
        >
          <ArrowDownIcon />
          {t('chat.jumpToBottom')}
        </button>
      )}
    </div>
  );
}

/** Dispatches one flat timeline item to its row component. */
export function TimelineRow({
  item,
  canFork = false,
  onFork,
}: {
  item: TimelineItem;
  canFork?: boolean;
  onFork?: (messageIndex: number, position: ForkPosition) => Promise<void>;
}) {
  switch (item.kind) {
    case 'user':
      return <UserMessage message={item.message} pending={item.pending} />;
    case 'assistant':
      return (
        <AssistantText
          messageIndex={item.messageIndex}
          forkPosition={item.forkPosition}
          reasoning={item.reasoning}
          text={item.text}
          streaming={item.streaming}
          forkEnabled={canFork}
          onFork={onFork}
        />
      );
    case 'tool':
      return (
        <ToolCallItem
          call={item.call}
          status={item.status}
          progress={item.progress}
          output={item.output}
          streamingArgs={item.streamingArgs}
        />
      );
    case 'tool-batch':
      return <ToolBatchItem tools={item.tools} />;
    case 'status':
      return <StatusLine step={item.step} />;
    case 'compaction':
      return <CompactionDivider phase={item.phase} message={item.message} />;
  }
}

const List = forwardRef<HTMLDivElement, ComponentPropsWithRef<'div'>>(
  function TimelineList(props, ref) {
    return (
      <div
        {...props}
        ref={ref}
        className={`${props.className ?? ''} ${styles.list}`}
      />
    );
  },
);

function BottomSpacer() {
  return (
    <div
      className={styles.bottomSpacer}
      data-timeline-tail-spacer=""
      aria-hidden="true"
    />
  );
}

/** Compact terminal-style spacing: dense for tool/status runs, airy for prose. */
function rowClass(items: TimelineItem[], index: number): string {
  const item = items[index];
  const prev = index > 0 ? items[index - 1] : undefined;
  const classes = [styles.row];
  if (index === 0) {
    classes.push(styles.rowFirst);
  } else if (item?.kind === 'user') {
    classes.push(styles.rowAfterUser);
  } else if (item?.kind === 'compaction') {
    classes.push(styles.rowCompaction);
  } else if (isActivityRow(item) && isActivityRow(prev)) {
    classes.push(styles.rowDense);
  }
  return classes.join(' ');
}

function isActivityRow(item: TimelineItem | undefined): boolean {
  return (
    item?.kind === 'tool' ||
    item?.kind === 'tool-batch' ||
    item?.kind === 'status'
  );
}

function isUpwardScrollKey(event: KeyboardEvent): boolean {
  return (
    event.key === 'ArrowUp' ||
    event.key === 'PageUp' ||
    event.key === 'Home' ||
    (event.key === ' ' && event.shiftKey)
  );
}
