import {
  type CSSProperties,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type {
  CapabilityMode,
  PublicProviderConfig,
  ReasoningEffort,
  SkillInvocation,
  SkillSummary,
  Usage,
} from '../../types/wire.ts';
import {
  ChevronIcon,
  CompactIcon,
  SendIcon,
  SparkIcon,
  StopIcon,
  WrenchIcon,
} from '../common/Icons.tsx';
import styles from './Composer.module.css';

interface ComposerProps {
  variant?: 'default' | 'welcome';
  disabled: boolean;
  busy: boolean;
  canStop: boolean;
  canConfigure: boolean;
  sessionActivated: boolean;
  canCompact: boolean;
  queuedCount: number;
  capabilityMode: CapabilityMode;
  profileId: string;
  providerProfiles: PublicProviderConfig[];
  model: string;
  reasoningEffort: ReasoningEffort | null;
  usage: Usage | null;
  skills: SkillSummary[];
  onSend: (text: string, skill?: SkillInvocation) => boolean;
  onStop: () => void;
  onSetCapabilityMode: (mode: CapabilityMode) => void;
  onSelectProvider: (profileId: string) => void;
  onSetModel: (model: string) => void;
  onSetReasoningEffort: (effort: ReasoningEffort | null) => void;
  onCompact: (instructions?: string) => boolean;
}

type OpenPanel = 'model' | null;

interface CommandPaletteItem {
  kind: 'command';
  name: 'compact';
  description: string;
  disabled: boolean;
}

interface SkillPaletteItem {
  kind: 'skill';
  name: string;
  description: string;
  argumentHint: string | null;
  skill: SkillSummary;
}

type PaletteItem = CommandPaletteItem | SkillPaletteItem;

type SlashSubmission =
  | { kind: 'compact'; instructions: string }
  | { kind: 'skill'; skill: SkillSummary; arguments: string }
  | { kind: 'prompt' };

const REASONING_OPTIONS = [
  ['', 'chat.reasoning.auto'],
  ['none', 'chat.reasoning.none'],
  ['minimal', 'chat.reasoning.minimal'],
  ['low', 'chat.reasoning.low'],
  ['medium', 'chat.reasoning.medium'],
  ['high', 'chat.reasoning.high'],
  ['xhigh', 'chat.reasoning.xhigh'],
  ['max', 'chat.reasoning.max'],
] as const;

export function Composer({
  variant = 'default',
  disabled,
  busy,
  canStop,
  canConfigure,
  sessionActivated,
  canCompact,
  queuedCount,
  capabilityMode,
  profileId,
  providerProfiles,
  model,
  reasoningEffort,
  usage,
  skills,
  onSend,
  onStop,
  onSetCapabilityMode,
  onSelectProvider,
  onSetModel,
  onSetReasoningEffort,
  onCompact,
}: ComposerProps) {
  const [value, setValue] = useState('');
  const [paletteDismissed, setPaletteDismissed] = useState(false);
  const [activePaletteIndex, setActivePaletteIndex] = useState(0);
  const [openPanel, setOpenPanel] = useState<OpenPanel>(null);
  const [contextHovered, setContextHovered] = useState(false);
  const [modelDraft, setModelDraft] = useState(model);
  const { t, locale } = useI18n();
  const composerRef = useRef<HTMLDivElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  // biome-ignore lint/correctness/useExhaustiveDependencies: value is the resize trigger; the effect only mutates the textarea ref
  useEffect(() => {
    const element = textareaRef.current;
    if (!element) return;
    element.style.height = 'auto';
    element.style.height = `${Math.min(element.scrollHeight, 220)}px`;
  }, [value]);

  useEffect(() => {
    setModelDraft(model);
  }, [model]);

  useEffect(() => {
    if (openPanel === null) return;
    const onPointerDown = (event: PointerEvent) => {
      if (
        composerRef.current &&
        !composerRef.current.contains(event.target as Node)
      ) {
        setOpenPanel(null);
      }
    };
    document.addEventListener('pointerdown', onPointerDown);
    return () => document.removeEventListener('pointerdown', onPointerDown);
  }, [openPanel]);

  const paletteQuery = slashPaletteQuery(value);
  const paletteItems = useMemo<PaletteItem[]>(() => {
    if (paletteQuery === null) return [];
    const query = paletteQuery.toLocaleLowerCase();
    const items: PaletteItem[] = [];
    if ('compact'.includes(query)) {
      items.push({
        kind: 'command',
        name: 'compact',
        description: t('chat.command.compactDescription'),
        disabled: !canCompact,
      });
    }
    for (const skill of skills) {
      if (!skill.user_invocable) continue;
      const searchable = [
        skill.name,
        skill.display_name ?? '',
        skill.description,
        skill.when_to_use ?? '',
      ]
        .join(' ')
        .toLocaleLowerCase();
      if (query && !searchable.includes(query)) continue;
      items.push({
        kind: 'skill',
        name: skill.name,
        description: skill.description,
        argumentHint: skill.argument_hint ?? null,
        skill,
      });
    }
    return items;
  }, [canCompact, paletteQuery, skills, t]);
  const paletteOpen = !disabled && !paletteDismissed && paletteQuery !== null;

  useEffect(() => {
    setActivePaletteIndex((current) =>
      paletteItems.length === 0
        ? 0
        : Math.min(current, paletteItems.length - 1),
    );
  }, [paletteItems.length]);

  const clearValue = () => {
    setValue('');
    setPaletteDismissed(false);
    requestAnimationFrame(() => textareaRef.current?.focus());
  };

  const submit = () => {
    if (disabled) return;
    const trimmed = value.trim();
    if (!trimmed) return;
    const submission = parseSlashSubmission(trimmed, skills);
    if (submission.kind === 'compact') {
      if (!canCompact) return;
      if (onCompact(submission.instructions || undefined)) clearValue();
      return;
    }
    if (submission.kind === 'skill') {
      if (
        onSend(submission.arguments, {
          name: submission.skill.name,
        })
      ) {
        clearValue();
      }
      return;
    }
    if (onSend(trimmed)) clearValue();
  };

  const choosePaletteItem = (item: PaletteItem) => {
    setValue(`/${item.name} `);
    setPaletteDismissed(true);
    setOpenPanel(null);
    requestAnimationFrame(() => textareaRef.current?.focus());
  };

  const selectProfile = (profile: PublicProviderConfig) => {
    if (!canConfigure) return;
    if (sessionActivated) {
      setModelDraft(profile.model);
      setOpenPanel(null);
      if (profile.model !== model) onSetModel(profile.model);
      requestAnimationFrame(() => textareaRef.current?.focus());
      return;
    }
    if (profile.profile_id === profileId) return;
    if (value.trim() && !window.confirm(t('chat.provider.discardDraft'))) {
      return;
    }
    setValue('');
    setOpenPanel(null);
    onSelectProvider(profile.profile_id);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (paletteOpen) {
      if (event.key === 'ArrowDown') {
        event.preventDefault();
        if (paletteItems.length > 0) {
          setActivePaletteIndex((activePaletteIndex + 1) % paletteItems.length);
        }
        return;
      }
      if (event.key === 'ArrowUp') {
        event.preventDefault();
        if (paletteItems.length > 0) {
          setActivePaletteIndex(
            (activePaletteIndex - 1 + paletteItems.length) %
              paletteItems.length,
          );
        }
        return;
      }
      if (
        (event.key === 'Enter' || event.key === 'Tab') &&
        !event.shiftKey &&
        paletteItems[activePaletteIndex]
      ) {
        event.preventDefault();
        choosePaletteItem(paletteItems[activePaletteIndex]);
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        setPaletteDismissed(true);
        return;
      }
    }
    if (
      event.key === 'Enter' &&
      !event.shiftKey &&
      !event.nativeEvent.isComposing
    ) {
      event.preventDefault();
      submit();
    }
  };

  const placeholder = disabled
    ? t('chat.composer.placeholderUnavailable')
    : busy
      ? t('chat.composer.placeholderQueue')
      : t('chat.composer.placeholder');
  const context = usage?.context ?? null;
  const contextPct =
    context && context.max_tokens > 0
      ? Math.min(100, (context.used_tokens / context.max_tokens) * 100)
      : null;
  const contextPctLabel = formatPercent(contextPct ?? 0, locale);
  const cacheHitPct =
    usage && usage.cumulative.input_tokens > 0
      ? Math.min(
          100,
          (usage.cumulative.cached_input_tokens /
            usage.cumulative.input_tokens) *
            100,
        )
      : null;
  const slashSubmission = parseSlashSubmission(value.trim(), skills);
  const submitDisabled =
    disabled ||
    value.trim().length === 0 ||
    (slashSubmission.kind === 'compact' && !canCompact);
  const commandItems = paletteItems.filter(
    (item): item is CommandPaletteItem => item.kind === 'command',
  );
  const skillItems = paletteItems.filter(
    (item): item is SkillPaletteItem => item.kind === 'skill',
  );
  const selectedModelProfileId = sessionActivated
    ? (
        providerProfiles.find(
          (profile) =>
            profile.profile_id === profileId && profile.model === model,
        ) ?? providerProfiles.find((profile) => profile.model === model)
      )?.profile_id
    : profileId;

  return (
    <footer
      className={`${styles.dock} ${variant === 'welcome' ? styles.welcomeDock : ''}`}
    >
      <div
        ref={composerRef}
        className={`${styles.composer} ${variant === 'welcome' ? styles.welcomeComposer : ''} ${disabled ? styles.composerDisabled : ''}`}
      >
        {paletteOpen && (
          <div
            id="composer-command-palette"
            className={styles.palette}
            role="listbox"
            aria-label={t('chat.command.palette')}
          >
            {commandItems.length > 0 && (
              <PaletteGroup
                label={t('chat.command.commands')}
                items={commandItems}
                startIndex={0}
                activeIndex={activePaletteIndex}
                onChoose={choosePaletteItem}
              />
            )}
            {skillItems.length > 0 && (
              <PaletteGroup
                label={t('chat.command.skills')}
                items={skillItems}
                startIndex={commandItems.length}
                activeIndex={activePaletteIndex}
                onChoose={choosePaletteItem}
              />
            )}
            {paletteItems.length === 0 && (
              <div className={styles.paletteEmpty}>
                {t('chat.command.noMatches')}
              </div>
            )}
            <div className={styles.paletteHint}>{t('chat.command.hint')}</div>
          </div>
        )}

        {contextHovered && (
          <div
            id="composer-context-usage"
            className={styles.controlPopover}
            role="tooltip"
          >
            <div className={styles.popoverHeader}>
              <strong>{t('chat.context.title')}</strong>
              <span>
                {context
                  ? `${formatTokens(context.used_tokens, locale)} / ${formatTokens(
                      context.max_tokens,
                      locale,
                    )} (${contextPctLabel}%)`
                  : t('chat.context.unavailable')}
              </span>
            </div>
            <div className={styles.usageTrack}>
              <i
                className={
                  contextPct !== null && contextPct > 85
                    ? styles.contextHigh
                    : ''
                }
                style={{ width: `${contextPct ?? 0}%` }}
              />
            </div>
            {context ? (
              <>
                <div className={styles.contextStats}>
                  <span>{t('chat.context.remaining')}</span>
                  <strong>
                    {formatTokens(context.remaining_tokens, locale)}
                  </strong>
                  <span>{t('chat.context.cachedInput')}</span>
                  <strong>
                    {cacheHitPct === null ? '—' : `${Math.round(cacheHitPct)}%`}
                  </strong>
                  <span>{t('chat.context.lastInput')}</span>
                  <strong>
                    {usage?.last
                      ? formatTokens(usage.last.input_tokens, locale)
                      : '—'}
                  </strong>
                  <span>{t('chat.context.lastOutput')}</span>
                  <strong>
                    {usage?.last
                      ? formatTokens(usage.last.output_tokens, locale)
                      : '—'}
                  </strong>
                </div>
                <p className={styles.popoverFootnote}>
                  {t('chat.context.measuredHint')}
                </p>
              </>
            ) : (
              <p className={styles.popoverCopy}>
                {t('chat.context.unavailableHint')}
              </p>
            )}
          </div>
        )}

        {openPanel === 'model' && (
          <form
            className={`${styles.controlPopover} ${styles.modelPopover}`}
            onSubmit={(event) => {
              event.preventDefault();
              const nextModel = modelDraft.trim();
              if (!nextModel || !canConfigure) return;
              if (nextModel !== model) onSetModel(nextModel);
              setOpenPanel(null);
            }}
          >
            <section className={styles.providerSection}>
              <strong className={styles.providerLabel}>
                {t('chat.provider.label')}
              </strong>
              {providerProfiles.length > 0 ? (
                <div
                  className={styles.providerChoices}
                  role="listbox"
                  aria-label={t('chat.provider.label')}
                >
                  {providerProfiles.map((profile) => {
                    const active =
                      profile.profile_id === selectedModelProfileId;
                    return (
                      <button
                        type="button"
                        role="option"
                        aria-selected={active}
                        aria-label={`${profile.profile_id}: ${profile.model}`}
                        className={`${styles.providerChoice} ${
                          active ? styles.providerChoiceActive : ''
                        }`}
                        key={profile.profile_id}
                        disabled={!canConfigure}
                        onClick={() => selectProfile(profile)}
                      >
                        <span className={styles.providerChoiceCopy}>
                          <strong>{profile.profile_id}</strong>
                          <small>{profile.model}</small>
                        </span>
                        <i aria-hidden="true" />
                      </button>
                    );
                  })}
                </div>
              ) : (
                <p className={styles.providerEmpty}>
                  {t('chat.provider.empty')}
                </p>
              )}
              <p className={styles.providerHint}>
                {t(
                  sessionActivated
                    ? 'chat.provider.nextRequestHint'
                    : 'chat.provider.switchHint',
                )}
              </p>
            </section>

            <div className={styles.modelOverride}>
              <label htmlFor="composer-model">{t('chat.model.label')}</label>
              <div className={styles.modelOverrideRow}>
                <input
                  id="composer-model"
                  value={modelDraft}
                  disabled={!canConfigure}
                  onChange={(event) => setModelDraft(event.target.value)}
                />
                <button
                  type="submit"
                  disabled={!canConfigure || modelDraft.trim().length === 0}
                >
                  {t('chat.model.apply')}
                </button>
              </div>
            </div>
          </form>
        )}

        <textarea
          ref={textareaRef}
          className={styles.textarea}
          value={value}
          rows={1}
          disabled={disabled}
          placeholder={placeholder}
          aria-label={t('chat.composer.ariaLabel')}
          aria-controls={paletteOpen ? 'composer-command-palette' : undefined}
          onChange={(event) => {
            setValue(event.target.value);
            setPaletteDismissed(false);
            setOpenPanel(null);
          }}
          onKeyDown={onKeyDown}
        />

        <div className={styles.footer}>
          <div className={styles.leftControls}>
            <label
              className={`${styles.selectControl} ${styles.capabilityControl}`}
              title={t('chat.capability.title')}
            >
              <span className={styles.capabilityDot} />
              <select
                aria-label={t('chat.capability.label')}
                value={capabilityMode}
                disabled={!canConfigure}
                onChange={(event) =>
                  onSetCapabilityMode(event.target.value as CapabilityMode)
                }
              >
                <option value="read_only">
                  {t('chat.capability.readOnly')}
                </option>
                <option value="workspace_edit">
                  {t('chat.capability.workspaceEdit')}
                </option>
                <option value="full_access">
                  {t('chat.capability.fullAccess')}
                </option>
              </select>
            </label>
            {queuedCount > 0 && (
              <span className={styles.queue}>
                {t('chat.composer.queued', { count: queuedCount })}
              </span>
            )}
          </div>

          <div className={styles.actions}>
            <button
              type="button"
              className={styles.contextButton}
              onMouseEnter={() => {
                setContextHovered(true);
                setOpenPanel(null);
              }}
              onMouseLeave={() => setContextHovered(false)}
              onFocus={() => {
                setContextHovered(true);
                setOpenPanel(null);
              }}
              onBlur={() => setContextHovered(false)}
              aria-describedby={
                contextHovered ? 'composer-context-usage' : undefined
              }
              aria-label={t('chat.context.title')}
            >
              <span
                className={styles.contextRing}
                style={
                  {
                    '--context-pct': `${contextPct ?? 0}%`,
                  } as CSSProperties
                }
              >
                <i />
              </span>
              <span>{contextPct === null ? '—' : `${contextPctLabel}%`}</span>
            </button>

            <button
              type="button"
              className={styles.modelButton}
              disabled={!canConfigure}
              onClick={() =>
                setOpenPanel((current) =>
                  current === 'model' ? null : 'model',
                )
              }
              aria-expanded={openPanel === 'model'}
              aria-label={`${t('chat.provider.label')}: ${profileId}; ${t(
                'chat.model.label',
              )}: ${model}`}
              title={t('chat.model.label')}
            >
              <span className={styles.modelButtonProfile}>{profileId}</span>
              <span className={styles.modelButtonModel}>{model}</span>
              <ChevronIcon />
            </button>

            <label
              className={styles.reasoningControl}
              title={t('chat.reasoning.label')}
            >
              <SparkIcon />
              <select
                aria-label={t('chat.reasoning.label')}
                value={reasoningEffort ?? ''}
                disabled={!canConfigure}
                onChange={(event) =>
                  onSetReasoningEffort(
                    (event.target.value || null) as ReasoningEffort | null,
                  )
                }
              >
                {REASONING_OPTIONS.map(([value, label]) => (
                  <option value={value} key={value || 'auto'}>
                    {t(label)}
                  </option>
                ))}
              </select>
            </label>

            {canStop && (
              <button
                type="button"
                className={styles.stopButton}
                onClick={onStop}
                title={t('chat.composer.stopTitle')}
              >
                <StopIcon />
                <span>{t('chat.stop')}</span>
              </button>
            )}
            <button
              type="button"
              className={styles.sendButton}
              onClick={submit}
              disabled={submitDisabled}
              title={
                busy ? t('chat.composer.queue') : t('chat.composer.sendTitle')
              }
              aria-label={
                busy ? t('chat.composer.queue') : t('chat.composer.send')
              }
            >
              <SendIcon />
            </button>
          </div>
        </div>
      </div>
      <p className={styles.disclaimer}>{t('chat.composer.disclaimer')}</p>
    </footer>
  );
}

function PaletteGroup({
  label,
  items,
  startIndex,
  activeIndex,
  onChoose,
}: {
  label: string;
  items: PaletteItem[];
  startIndex: number;
  activeIndex: number;
  onChoose: (item: PaletteItem) => void;
}) {
  return (
    <div className={styles.paletteGroup}>
      <div className={styles.paletteLabel}>{label}</div>
      {items.map((item, index) => {
        const absoluteIndex = startIndex + index;
        return (
          <button
            type="button"
            role="option"
            aria-selected={absoluteIndex === activeIndex}
            aria-disabled={item.kind === 'command' && item.disabled}
            className={`${styles.paletteItem} ${
              absoluteIndex === activeIndex ? styles.paletteItemActive : ''
            }`}
            key={`${item.kind}:${item.name}`}
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => onChoose(item)}
          >
            <span className={styles.paletteIcon}>
              {item.kind === 'command' ? <CompactIcon /> : <WrenchIcon />}
            </span>
            <span className={styles.paletteText}>
              <strong>/{item.name}</strong>
              <small>{item.description}</small>
              {item.kind === 'skill' && item.argumentHint && (
                <em>{item.argumentHint}</em>
              )}
            </span>
          </button>
        );
      })}
    </div>
  );
}

function slashPaletteQuery(value: string): string | null {
  if (!value.startsWith('/')) return null;
  const command = value.slice(1);
  if (/\s/.test(command)) return null;
  return command;
}

function parseSlashSubmission(
  value: string,
  skills: SkillSummary[],
): SlashSubmission {
  const match = /^\/([^\s]+)(?:\s+([\s\S]*))?$/.exec(value);
  if (!match) return { kind: 'prompt' };
  const name = match[1]?.toLocaleLowerCase() ?? '';
  const argumentsValue = match[2]?.trim() ?? '';
  if (name === 'compact') {
    return { kind: 'compact', instructions: argumentsValue };
  }
  const skill = skills.find(
    (entry) => entry.user_invocable && entry.name.toLocaleLowerCase() === name,
  );
  return skill
    ? { kind: 'skill', skill, arguments: argumentsValue }
    : { kind: 'prompt' };
}

function formatTokens(value: number, locale: 'en' | 'zh'): string {
  return new Intl.NumberFormat(locale === 'zh' ? 'zh-CN' : 'en', {
    notation: 'compact',
    maximumFractionDigits: 1,
  }).format(value);
}

function formatPercent(value: number, locale: 'en' | 'zh'): string {
  return new Intl.NumberFormat(locale === 'zh' ? 'zh-CN' : 'en', {
    minimumFractionDigits: 1,
    maximumFractionDigits: 1,
  }).format(value);
}
