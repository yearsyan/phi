import { useCallback, useEffect, useRef, useState } from 'react';
import { getProvider, listProviders, putProvider } from '../../api/http.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { TranslationKey } from '../../i18n/translations.ts';
import type {
  CapabilityMode,
  ProviderKind,
  PublicProviderConfig,
  PutProviderRequest,
  ReasoningEffort,
} from '../../types/wire.ts';
import { CloseIcon } from '../common/Icons.tsx';
import styles from './SettingsModal.module.css';

const PROVIDERS: ProviderKind[] = [
  'openai_chat',
  'openai_responses',
  'anthropic',
];
const EFFORTS: ReasoningEffort[] = [
  'none',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
];

interface SettingsModalProps {
  authKey: string;
  profileId: string;
  agentProfileId: string;
  capabilityMode: CapabilityMode | null;
  onClose: () => void;
  onSaveAuthKey: (key: string) => void;
  onSaveProfileId: (id: string) => void;
  onSaveAgentProfileId: (id: string) => void;
  onSaveCapabilityMode: (mode: CapabilityMode | null) => void;
  onConfigured: () => void;
}

interface ProfileFormState {
  profileId: string;
  provider: ProviderKind;
  apiKey: string;
  baseUrl: string;
  model: string;
  maxOutputTokens: string;
  maxContextTokens: string;
  temperature: string;
  reasoningEffort: ReasoningEffort | '';
  maxRetries: string;
  requestTimeoutSecs: string;
  streamIdleTimeoutSecs: string;
}

type BuildResult = PutProviderRequest | { errorKey: TranslationKey };

const emptyForm = (profileId: string): ProfileFormState => ({
  profileId,
  provider: 'openai_chat',
  apiKey: '',
  baseUrl: '',
  model: '',
  maxOutputTokens: '',
  maxContextTokens: '128000',
  temperature: '',
  reasoningEffort: '',
  maxRetries: '10',
  requestTimeoutSecs: '30',
  streamIdleTimeoutSecs: '120',
});

function fromConfig(
  profileId: string,
  config: PublicProviderConfig,
): ProfileFormState {
  return {
    profileId,
    provider: config.provider,
    apiKey: '',
    baseUrl: config.base_url,
    model: config.model,
    maxOutputTokens: config.max_output_tokens?.toString() ?? '',
    maxContextTokens: config.max_context_tokens.toString(),
    temperature: config.temperature?.toString() ?? '',
    reasoningEffort: config.reasoning_effort ?? '',
    maxRetries: config.max_retries.toString(),
    requestTimeoutSecs: config.request_timeout_secs.toString(),
    streamIdleTimeoutSecs: config.stream_idle_timeout_secs.toString(),
  };
}

export function SettingsModal({
  authKey,
  profileId,
  agentProfileId,
  capabilityMode,
  onClose,
  onSaveAuthKey,
  onSaveProfileId,
  onSaveAgentProfileId,
  onSaveCapabilityMode,
  onConfigured,
}: SettingsModalProps) {
  const { t } = useI18n();
  const [localAuthKey, setLocalAuthKey] = useState(authKey);
  const [localAgentProfileId, setLocalAgentProfileId] =
    useState(agentProfileId);
  const [localCapabilityMode, setLocalCapabilityMode] = useState<
    CapabilityMode | ''
  >(capabilityMode ?? '');
  const [form, setForm] = useState<ProfileFormState>(() =>
    emptyForm(profileId),
  );
  const [availableProfiles, setAvailableProfiles] = useState<string[]>([
    profileId,
  ]);
  const [configured, setConfigured] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const loadRevision = useRef(0);

  const applyResponse = useCallback(
    (
      response: {
        configured: boolean;
        provider: PublicProviderConfig | null;
      },
      id: string,
    ) => {
      setConfigured(response.configured);
      setForm(
        response.configured && response.provider
          ? fromConfig(id, response.provider)
          : emptyForm(id),
      );
      setDirty(false);
    },
    [],
  );

  const loadProfile = useCallback(
    async (key: string, requestedId: string) => {
      const revision = ++loadRevision.current;
      setLoading(true);
      setError(null);
      setSaved(false);
      try {
        const listed = await listProviders(key);
        if (revision !== loadRevision.current) return;
        const ids = listed.providers.map((profile) => profile.profile_id);
        setAvailableProfiles(ids.length > 0 ? ids : ['default']);

        const response = await getProvider(key, requestedId);
        if (revision !== loadRevision.current) return;
        applyResponse(response, requestedId);
      } catch (loadError) {
        if (revision !== loadRevision.current) return;
        setConfigured(false);
        setForm((current) => ({
          ...emptyForm(requestedId),
          apiKey: current.apiKey,
        }));
        setDirty(false);
        setError(
          loadError instanceof Error ? loadError.message : String(loadError),
        );
      } finally {
        if (revision === loadRevision.current) setLoading(false);
      }
    },
    [applyResponse],
  );

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [onClose]);

  useEffect(() => {
    if (authKey.trim()) void loadProfile(authKey, profileId);
  }, [authKey, profileId, loadProfile]);

  const updateField = <K extends keyof ProfileFormState>(
    key: K,
    value: ProfileFormState[K],
  ) => {
    setForm((current) => ({ ...current, [key]: value }));
    setDirty(true);
    if (key === 'profileId') setConfigured(false);
    setSaved(false);
  };

  const handleLoad = async () => {
    const key = localAuthKey.trim();
    if (!key) {
      setError(t('settings.errors.authKeyRequired'));
      return;
    }
    const id = form.profileId.trim() || 'default';
    await loadProfile(key, id);
  };

  const buildBody = (): BuildResult => {
    const apiKey = form.apiKey.trim();
    if (!apiKey) return { errorKey: 'settings.errors.apiKeyRequiredOnWrite' };
    const baseUrl = form.baseUrl.trim();
    if (!baseUrl) return { errorKey: 'settings.errors.baseUrlRequired' };
    const model = form.model.trim();
    if (!model) return { errorKey: 'settings.errors.modelRequired' };
    const maxContext = positiveInteger(form.maxContextTokens);
    if (maxContext === null) {
      return { errorKey: 'settings.errors.maxContext' };
    }

    const maxRetries = nonNegativeInteger(form.maxRetries);
    const requestTimeout = positiveInteger(form.requestTimeoutSecs);
    const streamTimeout = positiveInteger(form.streamIdleTimeoutSecs);
    if (
      maxRetries === null ||
      requestTimeout === null ||
      streamTimeout === null
    ) {
      return { errorKey: 'settings.errors.advancedNumbers' };
    }

    const body: PutProviderRequest = {
      provider: form.provider,
      api_key: apiKey,
      base_url: baseUrl,
      model,
      max_context_tokens: maxContext,
      max_retries: maxRetries,
      request_timeout_secs: requestTimeout,
      stream_idle_timeout_secs: streamTimeout,
    };
    if (form.maxOutputTokens.trim()) {
      const maxOutput = positiveInteger(form.maxOutputTokens);
      if (maxOutput === null) {
        return { errorKey: 'settings.errors.maxOutput' };
      }
      body.max_output_tokens = maxOutput;
    }
    if (form.temperature.trim()) {
      const temperature = Number.parseFloat(form.temperature);
      if (!Number.isFinite(temperature)) {
        return { errorKey: 'settings.errors.temperature' };
      }
      body.temperature = temperature;
    }
    if (form.reasoningEffort) body.reasoning_effort = form.reasoningEffort;
    return body;
  };

  const handleSave = async () => {
    setError(null);
    setSaved(false);
    const key = localAuthKey.trim();
    if (!key) {
      setError(t('settings.errors.authKeyRequired'));
      return;
    }
    const id = form.profileId.trim() || 'default';

    if (configured && !dirty) {
      setSaving(true);
      try {
        const response = await getProvider(key, id);
        if (!response.configured || response.provider === null) {
          setConfigured(false);
          setError(t('settings.errors.profileNotConfigured'));
          return;
        }
        onSaveAuthKey(key);
        onSaveProfileId(id);
        onSaveAgentProfileId(localAgentProfileId);
        onSaveCapabilityMode(localCapabilityMode || null);
        onConfigured();
      } catch (saveError) {
        setError(
          saveError instanceof Error ? saveError.message : String(saveError),
        );
      } finally {
        setSaving(false);
      }
      return;
    }

    const body = buildBody();
    if ('errorKey' in body) {
      setError(t(body.errorKey));
      return;
    }

    setSaving(true);
    try {
      const response = await putProvider(key, id, body);
      applyResponse(response, id);
      onSaveAuthKey(key);
      onSaveProfileId(id);
      onSaveAgentProfileId(localAgentProfileId);
      onSaveCapabilityMode(localCapabilityMode || null);
      setSaved(true);
      onConfigured();
    } catch (saveError) {
      setError(
        saveError instanceof Error ? saveError.message : String(saveError),
      );
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className={styles.overlay}>
      <button
        type="button"
        className={styles.backdrop}
        aria-label={t('settings.closeLabel')}
        onClick={onClose}
      />
      <section
        className={styles.modal}
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
      >
        <header className={styles.header}>
          <div>
            <p>{t('settings.eyebrow')}</p>
            <h2 id="settings-title">{t('settings.title')}</h2>
          </div>
          <button
            type="button"
            className={styles.closeButton}
            onClick={onClose}
            aria-label={t('settings.closeLabel')}
          >
            <CloseIcon />
          </button>
        </header>

        <div className={styles.body}>
          <section className={styles.connectionCard}>
            <div className={styles.sectionHeading}>
              <div>
                <h3>{t('settings.daemonConnection')}</h3>
                <p>{t('settings.connectionCopy')}</p>
              </div>
              <span
                className={`${styles.status} ${
                  configured ? styles.statusReady : styles.statusIdle
                }`}
              >
                {configured
                  ? t('settings.configured')
                  : t('settings.notConfigured')}
              </span>
            </div>
            <div className={styles.inlineFields}>
              <label className={styles.field}>
                <span>{t('settings.authKey')}</span>
                <input
                  type="password"
                  value={localAuthKey}
                  placeholder={t('settings.authKeyPlaceholder')}
                  onChange={(event) => setLocalAuthKey(event.target.value)}
                />
              </label>
              <button
                type="button"
                className={styles.secondaryButton}
                onClick={() => void handleLoad()}
                disabled={loading || !localAuthKey.trim()}
              >
                {loading ? t('settings.loading') : t('settings.load')}
              </button>
            </div>
          </section>

          <section className={styles.section}>
            <div className={styles.sectionHeading}>
              <div>
                <h3>{t('settings.providerProfile')}</h3>
                <p>{t('settings.providerCopy')}</p>
              </div>
            </div>

            <div className={styles.twoColumns}>
              <label className={styles.field}>
                <span>{t('settings.profileId')}</span>
                <input
                  list="phi-profile-options"
                  value={form.profileId}
                  onChange={(event) =>
                    updateField('profileId', event.target.value)
                  }
                />
                <datalist id="phi-profile-options">
                  {availableProfiles.map((id) => (
                    <option key={id} value={id} />
                  ))}
                </datalist>
              </label>
              <label className={styles.field}>
                <span>{t('settings.providerAdapter')}</span>
                <select
                  value={form.provider}
                  onChange={(event) =>
                    updateField('provider', event.target.value as ProviderKind)
                  }
                >
                  {PROVIDERS.map((provider) => (
                    <option key={provider} value={provider}>
                      {provider}
                    </option>
                  ))}
                </select>
              </label>
            </div>

            <div className={styles.twoColumns}>
              <label className={styles.field}>
                <span>{t('settings.model')}</span>
                <input
                  value={form.model}
                  placeholder="model-name"
                  onChange={(event) => updateField('model', event.target.value)}
                />
              </label>
              <label className={styles.field}>
                <span>{t('settings.apiKey')}</span>
                <input
                  type="password"
                  value={form.apiKey}
                  placeholder={
                    configured
                      ? t('settings.apiKeyRequiredToUpdate')
                      : t('settings.apiKeyPlaceholder')
                  }
                  onChange={(event) =>
                    updateField('apiKey', event.target.value)
                  }
                />
              </label>
            </div>

            <label className={styles.field}>
              <span>{t('settings.baseUrl')}</span>
              <input
                value={form.baseUrl}
                placeholder="https://api.example.com/v1"
                onChange={(event) => updateField('baseUrl', event.target.value)}
              />
            </label>
          </section>

          <section className={styles.section}>
            <div className={styles.sectionHeading}>
              <div>
                <h3>{t('settings.sessionDefaults')}</h3>
                <p>{t('settings.sessionDefaultsCopy')}</p>
              </div>
            </div>
            <div className={styles.twoColumns}>
              <label className={styles.field}>
                <span>{t('settings.agentProfileId')}</span>
                <input
                  value={localAgentProfileId}
                  placeholder={t('settings.agentProfileIdPlaceholder')}
                  onChange={(event) =>
                    setLocalAgentProfileId(event.target.value)
                  }
                />
              </label>
              <label className={styles.field}>
                <span>{t('settings.capabilityMode')}</span>
                <select
                  value={localCapabilityMode}
                  onChange={(event) =>
                    setLocalCapabilityMode(
                      event.target.value as CapabilityMode | '',
                    )
                  }
                >
                  <option value="">
                    {t('settings.capabilityProfileDefault')}
                  </option>
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
          </section>

          <details className={styles.advanced}>
            <summary>{t('settings.advanced')}</summary>
            <div className={styles.advancedBody}>
              <div className={styles.threeColumns}>
                <NumberField
                  label={t('settings.maxContextTokens')}
                  value={form.maxContextTokens}
                  onChange={(value) => updateField('maxContextTokens', value)}
                />
                <NumberField
                  label={t('settings.maxOutputTokens')}
                  value={form.maxOutputTokens}
                  onChange={(value) => updateField('maxOutputTokens', value)}
                />
                <NumberField
                  label={t('settings.temperature')}
                  value={form.temperature}
                  step="0.1"
                  onChange={(value) => updateField('temperature', value)}
                />
              </div>
              <div className={styles.threeColumns}>
                <label className={styles.field}>
                  <span>{t('settings.reasoningEffort')}</span>
                  <select
                    value={form.reasoningEffort}
                    onChange={(event) =>
                      updateField(
                        'reasoningEffort',
                        event.target.value as ReasoningEffort | '',
                      )
                    }
                  >
                    <option value="">{t('settings.effortNone')}</option>
                    {EFFORTS.map((effort) => (
                      <option key={effort} value={effort}>
                        {effort}
                      </option>
                    ))}
                  </select>
                </label>
                <NumberField
                  label={t('settings.maxRetries')}
                  value={form.maxRetries}
                  onChange={(value) => updateField('maxRetries', value)}
                />
                <NumberField
                  label={t('settings.requestTimeoutSecs')}
                  value={form.requestTimeoutSecs}
                  onChange={(value) => updateField('requestTimeoutSecs', value)}
                />
              </div>
              <NumberField
                label={t('settings.streamIdleTimeoutSecs')}
                value={form.streamIdleTimeoutSecs}
                onChange={(value) =>
                  updateField('streamIdleTimeoutSecs', value)
                }
              />
            </div>
          </details>

          {configured && dirty && !form.apiKey.trim() && (
            <div className={styles.warning}>
              {t('settings.apiKeyUpdateWarning')}
            </div>
          )}
          {error && (
            <div className={styles.error} role="alert">
              {error}
            </div>
          )}
          {saved && <div className={styles.success}>{t('settings.saved')}</div>}
        </div>

        <footer className={styles.footer}>
          <p>{t('settings.footerHint')}</p>
          <div>
            <button
              type="button"
              className={styles.secondaryButton}
              onClick={onClose}
            >
              {t('settings.close')}
            </button>
            <button
              type="button"
              className={styles.primaryButton}
              onClick={() => void handleSave()}
              disabled={saving || !localAuthKey.trim()}
            >
              {saving ? t('settings.saving') : t('settings.save')}
            </button>
          </div>
        </footer>
      </section>
    </div>
  );
}

function NumberField({
  label,
  value,
  step,
  onChange,
}: {
  label: string;
  value: string;
  step?: string;
  onChange: (value: string) => void;
}) {
  return (
    <label className={styles.field}>
      <span>{label}</span>
      <input
        type="number"
        step={step}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}

function positiveInteger(value: string): number | null {
  const parsed = Number.parseInt(value, 10);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : null;
}

function nonNegativeInteger(value: string): number | null {
  const parsed = Number.parseInt(value, 10);
  return Number.isInteger(parsed) && parsed >= 0 ? parsed : null;
}
