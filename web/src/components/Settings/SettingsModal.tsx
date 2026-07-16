import { useEffect, useState } from 'react';
import { getProvider, listProviders, putProvider } from '../../api/http.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { TranslationKey } from '../../i18n/translations.ts';
import type {
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
  onClose: () => void;
  onSaveAuthKey: (key: string) => void;
  onSaveProfileId: (id: string) => void;
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
  keepApiKey: string,
): ProfileFormState {
  return {
    profileId,
    provider: config.provider,
    apiKey: keepApiKey,
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

/** Validation result: either a valid body or a translation key for the error. */
type BuildResult = PutProviderRequest | { errorKey: TranslationKey };

export function SettingsModal({
  authKey,
  profileId,
  onClose,
  onSaveAuthKey,
  onSaveProfileId,
  onConfigured,
}: SettingsModalProps) {
  const { t } = useI18n();
  const [localAuthKey, setLocalAuthKey] = useState(authKey);
  const [form, setForm] = useState<ProfileFormState>(() =>
    emptyForm(profileId),
  );
  const [availableProfiles, setAvailableProfiles] = useState<string[]>([
    profileId,
  ]);
  const [configured, setConfigured] = useState(false);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const loadProfile = async (key: string, id: string) => {
    setLoading(true);
    setError(null);
    try {
      const list = await listProfilesSafe(key);
      setAvailableProfiles(list);
      if (!list.includes(id) && list.length > 0) {
        const fallback = list[0];
        if (fallback === undefined) return;
        setForm((prev) => ({ ...prev, profileId: fallback }));
        const response = await getProvider(key, fallback);
        applyProviderResponse(response, fallback);
        return;
      }
      const response = await getProvider(key, id);
      applyProviderResponse(response, id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  const applyProviderResponse = (
    response: { configured: boolean; provider: PublicProviderConfig | null },
    id: string,
  ) => {
    setConfigured(response.configured);
    const provider = response.provider;
    if (response.configured && provider) {
      setForm((prev) => fromConfig(id, provider, prev.apiKey));
    } else {
      setForm((prev) => ({ ...emptyForm(id), apiKey: prev.apiKey }));
    }
  };

  // Reload the profile list + selected config whenever the auth key or profile
  // id changes. `loadProfile` is a component-local closure and is intentionally
  // excluded to avoid refetching on every render.
  // biome-ignore lint/correctness/useExhaustiveDependencies: loadProfile is covered by [authKey, profileId]
  useEffect(() => {
    if (!authKey) {
      setForm(emptyForm(profileId));
      return;
    }
    void loadProfile(authKey, profileId);
  }, [authKey, profileId]);

  const handleField = <K extends keyof ProfileFormState>(
    key: K,
    value: ProfileFormState[K],
  ) => {
    setForm((prev) => ({ ...prev, [key]: value }));
    setSaved(false);
  };

  const handleSelectProfile = async (id: string) => {
    handleField('profileId', id);
    if (authKey) {
      await loadProfile(authKey, id);
    }
  };

  const buildBody = (): BuildResult => {
    const apiKey = form.apiKey.trim();
    if (!apiKey && !configured) {
      return { errorKey: 'settings.errors.apiKeyRequired' };
    }
    const baseUrl = form.baseUrl.trim();
    if (!baseUrl) return { errorKey: 'settings.errors.baseUrlRequired' };
    const model = form.model.trim();
    if (!model) return { errorKey: 'settings.errors.modelRequired' };
    const maxContext = Number.parseInt(form.maxContextTokens, 10);
    if (!Number.isFinite(maxContext) || maxContext <= 0) {
      return { errorKey: 'settings.errors.maxContext' };
    }
    const maxOutput = form.maxOutputTokens.trim();
    const temperature = form.temperature.trim();
    const body: PutProviderRequest = {
      provider: form.provider,
      api_key: apiKey,
      base_url: baseUrl,
      model,
      max_context_tokens: maxContext,
      max_retries: Number.parseInt(form.maxRetries || '10', 10) || 10,
      request_timeout_secs:
        Number.parseInt(form.requestTimeoutSecs || '30', 10) || 30,
      stream_idle_timeout_secs:
        Number.parseInt(form.streamIdleTimeoutSecs || '120', 10) || 120,
    };
    if (maxOutput) body.max_output_tokens = Number.parseInt(maxOutput, 10);
    if (temperature) body.temperature = Number.parseFloat(temperature);
    if (form.reasoningEffort) body.reasoning_effort = form.reasoningEffort;
    return body;
  };

  const handleSave = async () => {
    setError(null);
    setSaved(false);

    const built = buildBody();
    if ('errorKey' in built) {
      setError(t(built.errorKey));
      return;
    }
    const savedAuthKey = localAuthKey.trim();
    if (!savedAuthKey) {
      setError(t('settings.errors.authKeyRequired'));
      return;
    }
    const savedProfileId = form.profileId.trim() || 'default';
    setSaving(true);
    try {
      const response = await putProvider(savedAuthKey, savedProfileId, built);
      onSaveAuthKey(savedAuthKey);
      onSaveProfileId(savedProfileId);
      onConfigured();
      setConfigured(response.configured);
      const provider = response.provider;
      if (response.configured && provider) {
        setForm((prev) => fromConfig(savedProfileId, provider, prev.apiKey));
      }
      setSaved(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
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
      <div
        className={styles.modal}
        role="dialog"
        aria-modal="true"
        aria-label={t('settings.title')}
      >
        <header className={styles.header}>
          <h2 className={styles.title}>{t('settings.title')}</h2>
          <button
            type="button"
            className={styles.closeBtn}
            onClick={onClose}
            aria-label={t('settings.closeLabel')}
          >
            <CloseIcon />
          </button>
        </header>

        <div className={styles.body}>
          <section className={styles.section}>
            <h3 className={styles.sectionTitle}>
              {t('settings.daemonConnection')}
            </h3>
            <label className={styles.field}>
              <span className={styles.label}>{t('settings.authKey')}</span>
              <input
                type="password"
                className={styles.input}
                value={localAuthKey}
                placeholder={t('settings.authKeyPlaceholder')}
                onChange={(event) => setLocalAuthKey(event.target.value)}
              />
              <span className={styles.hint}>{t('settings.authKeyHint')}</span>
            </label>
          </section>

          <section className={styles.section}>
            <h3 className={styles.sectionTitle}>
              {t('settings.providerProfile')}
            </h3>

            <div className={styles.row}>
              <label className={styles.field}>
                <span className={styles.label}>{t('settings.profileId')}</span>
                <select
                  className={styles.input}
                  value={form.profileId}
                  onChange={(event) =>
                    void handleSelectProfile(event.target.value)
                  }
                >
                  {availableProfiles.map((id) => (
                    <option key={id} value={id}>
                      {id}
                    </option>
                  ))}
                  {!availableProfiles.includes(form.profileId) && (
                    <option value={form.profileId}>{form.profileId}</option>
                  )}
                </select>
              </label>
              <div className={styles.field}>
                <span className={styles.label}>{t('settings.status')}</span>
                <span
                  className={`${styles.statusPill} ${configured ? styles.statusOk : styles.statusOff}`}
                >
                  {configured
                    ? t('settings.configured')
                    : t('settings.notConfigured')}
                </span>
              </div>
            </div>

            <div className={styles.row}>
              <label className={styles.field}>
                <span className={styles.label}>
                  {t('settings.providerAdapter')}
                </span>
                <select
                  className={styles.input}
                  value={form.provider}
                  onChange={(event) =>
                    handleField('provider', event.target.value as ProviderKind)
                  }
                >
                  {PROVIDERS.map((provider) => (
                    <option key={provider} value={provider}>
                      {provider}
                    </option>
                  ))}
                </select>
              </label>
              <label className={styles.field}>
                <span className={styles.label}>{t('settings.model')}</span>
                <input
                  className={styles.input}
                  value={form.model}
                  placeholder="model-name"
                  onChange={(event) => handleField('model', event.target.value)}
                />
              </label>
            </div>

            <label className={styles.field}>
              <span className={styles.label}>{t('settings.apiKey')}</span>
              <input
                type="password"
                className={styles.input}
                value={form.apiKey}
                placeholder={
                  configured
                    ? t('settings.apiKeyPlaceholderConfigured')
                    : t('settings.apiKeyPlaceholder')
                }
                onChange={(event) => handleField('apiKey', event.target.value)}
              />
            </label>

            <label className={styles.field}>
              <span className={styles.label}>{t('settings.baseUrl')}</span>
              <input
                className={styles.input}
                value={form.baseUrl}
                placeholder="https://api.example.com/v1"
                onChange={(event) => handleField('baseUrl', event.target.value)}
              />
            </label>

            <div className={styles.row}>
              <label className={styles.field}>
                <span className={styles.label}>
                  {t('settings.maxContextTokens')}
                </span>
                <input
                  className={styles.input}
                  type="number"
                  value={form.maxContextTokens}
                  onChange={(event) =>
                    handleField('maxContextTokens', event.target.value)
                  }
                />
              </label>
              <label className={styles.field}>
                <span className={styles.label}>
                  {t('settings.maxOutputTokens')}
                </span>
                <input
                  className={styles.input}
                  type="number"
                  value={form.maxOutputTokens}
                  onChange={(event) =>
                    handleField('maxOutputTokens', event.target.value)
                  }
                />
              </label>
            </div>

            <div className={styles.row}>
              <label className={styles.field}>
                <span className={styles.label}>
                  {t('settings.temperature')}
                </span>
                <input
                  className={styles.input}
                  type="number"
                  step="0.1"
                  value={form.temperature}
                  onChange={(event) =>
                    handleField('temperature', event.target.value)
                  }
                />
              </label>
              <label className={styles.field}>
                <span className={styles.label}>
                  {t('settings.reasoningEffort')}
                </span>
                <select
                  className={styles.input}
                  value={form.reasoningEffort}
                  onChange={(event) =>
                    handleField(
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
            </div>

            <div className={styles.row}>
              <label className={styles.field}>
                <span className={styles.label}>{t('settings.maxRetries')}</span>
                <input
                  className={styles.input}
                  type="number"
                  value={form.maxRetries}
                  onChange={(event) =>
                    handleField('maxRetries', event.target.value)
                  }
                />
              </label>
              <label className={styles.field}>
                <span className={styles.label}>
                  {t('settings.requestTimeoutSecs')}
                </span>
                <input
                  className={styles.input}
                  type="number"
                  value={form.requestTimeoutSecs}
                  onChange={(event) =>
                    handleField('requestTimeoutSecs', event.target.value)
                  }
                />
              </label>
              <label className={styles.field}>
                <span className={styles.label}>
                  {t('settings.streamIdleTimeoutSecs')}
                </span>
                <input
                  className={styles.input}
                  type="number"
                  value={form.streamIdleTimeoutSecs}
                  onChange={(event) =>
                    handleField('streamIdleTimeoutSecs', event.target.value)
                  }
                />
              </label>
            </div>
          </section>

          {error && <div className={styles.error}>{error}</div>}
          {saved && <div className={styles.success}>{t('settings.saved')}</div>}
        </div>

        <footer className={styles.footer}>
          <span className={styles.footerHint}>
            {loading ? t('settings.loading') : t('settings.footerHint')}
          </span>
          <div className={styles.footerActions}>
            <button
              type="button"
              className={styles.cancelBtn}
              onClick={onClose}
            >
              {t('settings.close')}
            </button>
            <button
              type="button"
              className={styles.saveBtn}
              onClick={() => void handleSave()}
              disabled={saving || !localAuthKey.trim()}
            >
              {saving ? t('settings.saving') : t('settings.save')}
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
}

async function listProfilesSafe(authKey: string): Promise<string[]> {
  try {
    const response = await listProviders(authKey);
    const ids = response.providers.map((provider) => provider.profile_id);
    return ids.length > 0 ? ids : ['default'];
  } catch {
    return ['default'];
  }
}
