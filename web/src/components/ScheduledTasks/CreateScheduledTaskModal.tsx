import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import {
  browseWorkspace,
  listAgentProfiles,
  listProviders,
} from '../../api/http.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type {
  CapabilityMode,
  CreateScheduledTaskRequest,
  PublicAgentProfile,
  PublicProviderConfig,
  ScheduledIntervalUnit,
  ScheduledWeekday,
} from '../../types/wire.ts';
import { WorkspacePicker } from '../Chat/WorkspacePicker.tsx';
import { ClockIcon, CloseIcon } from '../common/Icons.tsx';
import styles from './CreateScheduledTaskModal.module.css';

const WEEKDAYS: ScheduledWeekday[] = [
  'monday',
  'tuesday',
  'wednesday',
  'thursday',
  'friday',
  'saturday',
  'sunday',
];

interface CreateScheduledTaskModalProps {
  authKey: string;
  profileId: string;
  agentProfileId: string;
  capabilityMode: CapabilityMode | null;
  onClose: () => void;
  onCreate: (request: CreateScheduledTaskRequest) => Promise<void>;
}

export function CreateScheduledTaskModal({
  authKey,
  profileId,
  agentProfileId,
  capabilityMode,
  onClose,
  onCreate,
}: CreateScheduledTaskModalProps) {
  const { t } = useI18n();
  const nameRef = useRef<HTMLInputElement>(null);
  const [name, setName] = useState('');
  const [prompt, setPrompt] = useState('');
  const [workspace, setWorkspace] = useState<string | null>(null);
  const [selectedProfile, setSelectedProfile] = useState(profileId);
  const [selectedAgentProfile, setSelectedAgentProfile] = useState(
    agentProfileId.trim() || 'default',
  );
  const [selectedCapability, setSelectedCapability] = useState<
    CapabilityMode | ''
  >(capabilityMode ?? '');
  const [providers, setProviders] = useState<PublicProviderConfig[]>([]);
  const [agentProfiles, setAgentProfiles] = useState<PublicAgentProfile[]>([]);
  const [scheduleType, setScheduleType] = useState<'daily' | 'interval'>(
    'daily',
  );
  const [dailyTime, setDailyTime] = useState('09:00');
  const [weekdays, setWeekdays] = useState<ScheduledWeekday[]>(() =>
    WEEKDAYS.slice(0, 5),
  );
  const [intervalEvery, setIntervalEvery] = useState(1);
  const [intervalUnit, setIntervalUnit] =
    useState<ScheduledIntervalUnit>('hours');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const timezone = resolvedTimezone();

  useEffect(() => {
    nameRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !submitting) onClose();
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [onClose, submitting]);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([
      browseWorkspace(authKey),
      listProviders(authKey),
      listAgentProfiles(authKey),
    ])
      .then(([workspaceResponse, providerResponse, agentProfileResponse]) => {
        if (cancelled) return;
        setWorkspace(workspaceResponse.path);
        setProviders(providerResponse.providers);
        setAgentProfiles(agentProfileResponse.agent_profiles);
      })
      .catch((loadError) => {
        if (!cancelled) {
          setError(
            loadError instanceof Error ? loadError.message : String(loadError),
          );
        }
      });
    return () => {
      cancelled = true;
    };
  }, [authKey]);

  const toggleWeekday = (weekday: ScheduledWeekday) => {
    setWeekdays((current) =>
      current.includes(weekday)
        ? current.filter((item) => item !== weekday)
        : WEEKDAYS.filter(
            (candidate) => current.includes(candidate) || candidate === weekday,
          ),
    );
  };

  const scheduleIsValid =
    scheduleType === 'daily'
      ? dailyTime.length > 0 && weekdays.length > 0
      : Number.isInteger(intervalEvery) &&
        intervalEvery > 0 &&
        intervalEvery <= maxIntervalValue(intervalUnit);
  const canCreate =
    name.trim().length > 0 && prompt.trim().length > 0 && scheduleIsValid;

  const submit = async () => {
    if (!canCreate || submitting) return;
    setSubmitting(true);
    setError(null);
    const schedule =
      scheduleType === 'daily'
        ? {
            type: 'daily' as const,
            time: dailyTime,
            weekdays,
            timezone,
          }
        : {
            type: 'interval' as const,
            every: intervalEvery,
            unit: intervalUnit,
          };
    const request: CreateScheduledTaskRequest = {
      name: name.trim(),
      prompt: prompt.trim(),
      profile_id: selectedProfile,
      agent_profile_id: selectedAgentProfile,
      schedule,
    };
    if (workspace) request.workspace = workspace;
    if (selectedCapability) request.capability_mode = selectedCapability;
    try {
      await onCreate(request);
    } catch (createError) {
      setError(
        createError instanceof Error
          ? createError.message
          : String(createError),
      );
      setSubmitting(false);
    }
  };

  return createPortal(
    <div className={styles.backdrop}>
      <div
        className={styles.modal}
        role="dialog"
        aria-modal="true"
        aria-labelledby="create-scheduled-task-title"
      >
        <header className={styles.header}>
          <div>
            <h2 id="create-scheduled-task-title">
              {t('scheduled.modal.title')}
            </h2>
            <p>{t('scheduled.modal.subtitle')}</p>
          </div>
          <button
            type="button"
            onClick={onClose}
            disabled={submitting}
            aria-label={t('scheduled.modal.close')}
          >
            <CloseIcon />
          </button>
        </header>

        <form
          className={styles.form}
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <div className={styles.notice}>
            <ClockIcon />
            <span>{t('scheduled.modal.notice')}</span>
          </div>

          <label className={styles.field}>
            <span>{t('scheduled.field.name')}</span>
            <input
              ref={nameRef}
              aria-label={t('scheduled.field.name')}
              value={name}
              maxLength={100}
              onChange={(event) => setName(event.target.value)}
              placeholder={t('scheduled.field.namePlaceholder')}
              disabled={submitting}
            />
            <small>{name.length}/100</small>
          </label>

          <div className={styles.field}>
            <span>{t('scheduled.field.workspace')}</span>
            <p>{t('scheduled.field.workspaceHint')}</p>
            <div className={styles.workspacePicker}>
              <WorkspacePicker
                authKey={authKey}
                workspace={workspace}
                disabled={submitting}
                onSelect={setWorkspace}
              />
            </div>
          </div>

          <label className={styles.field}>
            <span>{t('scheduled.field.prompt')}</span>
            <textarea
              aria-label={t('scheduled.field.prompt')}
              value={prompt}
              maxLength={20_000}
              onChange={(event) => setPrompt(event.target.value)}
              placeholder={t('scheduled.field.promptPlaceholder')}
              disabled={submitting}
            />
            <small>{prompt.length}/20000</small>
          </label>

          <div className={styles.optionsGrid}>
            <label className={styles.field}>
              <span>{t('scheduled.field.provider')}</span>
              <select
                value={selectedProfile}
                onChange={(event) => setSelectedProfile(event.target.value)}
                disabled={submitting}
              >
                {optionValues(
                  selectedProfile,
                  providers.map((provider) => provider.profile_id),
                ).map((id) => {
                  const provider = providers.find(
                    (candidate) => candidate.profile_id === id,
                  );
                  return (
                    <option value={id} key={id}>
                      {provider ? `${id} · ${provider.model}` : id}
                    </option>
                  );
                })}
              </select>
            </label>

            <label className={styles.field}>
              <span>{t('scheduled.field.agentProfile')}</span>
              <select
                value={selectedAgentProfile}
                onChange={(event) =>
                  setSelectedAgentProfile(event.target.value)
                }
                disabled={submitting}
              >
                {optionValues(
                  selectedAgentProfile,
                  agentProfiles.map((profile) => profile.agent_profile_id),
                ).map((id) => (
                  <option value={id} key={id}>
                    {id}
                  </option>
                ))}
              </select>
            </label>

            <label className={styles.field}>
              <span>{t('scheduled.field.capability')}</span>
              <select
                value={selectedCapability}
                onChange={(event) =>
                  setSelectedCapability(
                    event.target.value as CapabilityMode | '',
                  )
                }
                disabled={submitting}
              >
                <option value="">{t('scheduled.capability.profile')}</option>
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
          </div>

          <section className={styles.scheduleSection}>
            <div className={styles.scheduleHeader}>
              <strong>{t('scheduled.field.schedule')}</strong>
              <div className={styles.tabs} role="tablist">
                <button
                  type="button"
                  role="tab"
                  aria-selected={scheduleType === 'daily'}
                  onClick={() => setScheduleType('daily')}
                >
                  {t('scheduled.schedule.dailyTab')}
                </button>
                <button
                  type="button"
                  role="tab"
                  aria-selected={scheduleType === 'interval'}
                  onClick={() => setScheduleType('interval')}
                >
                  {t('scheduled.schedule.intervalTab')}
                </button>
              </div>
            </div>

            {scheduleType === 'daily' ? (
              <div className={styles.dailyControls}>
                <input
                  type="time"
                  value={dailyTime}
                  onChange={(event) => setDailyTime(event.target.value)}
                  disabled={submitting}
                  aria-label={t('scheduled.schedule.time')}
                />
                <div className={styles.weekdays}>
                  {WEEKDAYS.map((weekday) => (
                    <button
                      type="button"
                      key={weekday}
                      className={
                        weekdays.includes(weekday) ? styles.weekdayActive : ''
                      }
                      aria-label={t(`scheduled.weekday.${weekday}`)}
                      aria-pressed={weekdays.includes(weekday)}
                      onClick={() => toggleWeekday(weekday)}
                    >
                      {t(`scheduled.weekday.short.${weekday}`)}
                    </button>
                  ))}
                </div>
              </div>
            ) : (
              <div className={styles.intervalControls}>
                <span>{t('scheduled.schedule.every')}</span>
                <input
                  type="number"
                  min={1}
                  max={maxIntervalValue(intervalUnit)}
                  value={intervalEvery}
                  onChange={(event) =>
                    setIntervalEvery(Number(event.target.value))
                  }
                  disabled={submitting}
                  aria-label={t('scheduled.schedule.every')}
                />
                <select
                  value={intervalUnit}
                  onChange={(event) =>
                    setIntervalUnit(event.target.value as ScheduledIntervalUnit)
                  }
                  disabled={submitting}
                  aria-label={t('scheduled.schedule.unit')}
                >
                  <option value="minutes">{t('scheduled.unit.minutes')}</option>
                  <option value="hours">{t('scheduled.unit.hours')}</option>
                  <option value="days">{t('scheduled.unit.days')}</option>
                </select>
              </div>
            )}
            {scheduleType === 'daily' && (
              <small className={styles.timezone}>
                {t('scheduled.schedule.timezone', { timezone })}
              </small>
            )}
          </section>

          {error && (
            <div className={styles.error} role="alert">
              {error}
            </div>
          )}

          <footer className={styles.footer}>
            <button
              type="button"
              className={styles.cancelButton}
              onClick={onClose}
              disabled={submitting}
            >
              {t('scheduled.modal.cancel')}
            </button>
            <button
              type="submit"
              className={styles.submitButton}
              disabled={!canCreate || submitting}
            >
              {submitting
                ? t('scheduled.modal.creating')
                : t('scheduled.modal.create')}
            </button>
          </footer>
        </form>
      </div>
    </div>,
    document.body,
  );
}

function resolvedTimezone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
}

function optionValues(selected: string, values: string[]): string[] {
  return Array.from(new Set([selected, ...values])).filter(Boolean);
}

function maxIntervalValue(unit: ScheduledIntervalUnit): number {
  switch (unit) {
    case 'minutes':
      return 10 * 366 * 24 * 60;
    case 'hours':
      return 10 * 366 * 24;
    case 'days':
      return 10 * 366;
  }
}
