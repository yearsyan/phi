import { useCallback, useEffect, useRef, useState } from 'react';
import { listMcpProfiles, putMcpProfile } from '../../api/http.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type {
  PublicMcpProfile,
  PutMcpProfileRequest,
} from '../../types/wire.ts';
import { PlusIcon } from '../common/Icons.tsx';
import styles from './ProfileManager.module.css';

interface McpProfileManagerProps {
  authKey: string;
  onDirtyChange: (dirty: boolean) => void;
}

type TransportType = 'http' | 'stdio';

interface McpProfileForm {
  id: string;
  transportType: TransportType;
  toolNamePrefix: string;
  connectTimeoutSecs: string;
  requestTimeoutSecs: string;
  maxOutputLines: string;
  maxOutputBytes: string;
  url: string;
  bearerToken: string;
  headers: string;
  allowStateless: boolean;
  reinitializeOnExpiredSession: boolean;
  command: string;
  args: string;
  currentDir: string;
  env: string;
  clearEnv: boolean;
}

function emptyForm(): McpProfileForm {
  return {
    id: '',
    transportType: 'http',
    toolNamePrefix: '',
    connectTimeoutSecs: '30',
    requestTimeoutSecs: '60',
    maxOutputLines: '2000',
    maxOutputBytes: '51200',
    url: '',
    bearerToken: '',
    headers: '',
    allowStateless: true,
    reinitializeOnExpiredSession: true,
    command: '',
    args: '',
    currentDir: '',
    env: '',
    clearEnv: false,
  };
}

function fromProfile(profile: PublicMcpProfile): McpProfileForm {
  const common = {
    ...emptyForm(),
    id: profile.mcp_profile_id,
    transportType: profile.transport.type,
    toolNamePrefix: profile.tool_name_prefix,
    connectTimeoutSecs: profile.connect_timeout_secs.toString(),
    requestTimeoutSecs: profile.request_timeout_secs?.toString() ?? '',
    maxOutputLines: profile.max_output_lines.toString(),
    maxOutputBytes: profile.max_output_bytes.toString(),
  };
  if (profile.transport.type === 'http') {
    return {
      ...common,
      url: profile.transport.url,
      allowStateless: profile.transport.allow_stateless,
      reinitializeOnExpiredSession:
        profile.transport.reinitialize_on_expired_session,
    };
  }
  return {
    ...common,
    command: profile.transport.command,
    args: profile.transport.args.join('\n'),
    currentDir: profile.transport.current_dir ?? '',
    clearEnv: profile.transport.clear_env,
  };
}

export function McpProfileManager({
  authKey,
  onDirtyChange,
}: McpProfileManagerProps) {
  const { t } = useI18n();
  const [profiles, setProfiles] = useState<PublicMcpProfile[]>([]);
  const [selected, setSelected] = useState<PublicMcpProfile | null>(null);
  const [form, setForm] = useState<McpProfileForm>(emptyForm);
  const [configured, setConfigured] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const loadRevision = useRef(0);

  const setDirtyState = useCallback(
    (value: boolean) => {
      setDirty(value);
      onDirtyChange(value);
    },
    [onDirtyChange],
  );

  const applyProfile = useCallback(
    (profile: PublicMcpProfile) => {
      setSelected(profile);
      setForm(fromProfile(profile));
      setConfigured(true);
      setDirtyState(false);
      setError(null);
      setSaved(false);
    },
    [setDirtyState],
  );

  const load = useCallback(async () => {
    if (!authKey.trim()) return;
    const currentRevision = ++loadRevision.current;
    setLoading(true);
    setError(null);
    try {
      const response = await listMcpProfiles(authKey.trim());
      if (loadRevision.current !== currentRevision) return;
      setProfiles(response.mcp_profiles);
      const next = response.mcp_profiles[0];
      if (next) {
        applyProfile(next);
      } else {
        setSelected(null);
        setForm(emptyForm());
        setConfigured(false);
        setDirtyState(false);
      }
    } catch (loadError) {
      if (loadRevision.current !== currentRevision) return;
      setProfiles([]);
      setError(
        loadError instanceof Error ? loadError.message : String(loadError),
      );
    } finally {
      if (loadRevision.current === currentRevision) setLoading(false);
    }
  }, [applyProfile, authKey, setDirtyState]);

  useEffect(() => {
    void load();
  }, [load]);

  const update = <K extends keyof McpProfileForm>(
    key: K,
    value: McpProfileForm[K],
  ) => {
    setForm((current) => ({ ...current, [key]: value }));
    setDirtyState(true);
    setSaved(false);
  };

  const confirmDiscard = () =>
    !dirty || window.confirm(t('settings.mcp.discardChanges'));

  const selectProfile = (profile: PublicMcpProfile) => {
    if (configured && profile.mcp_profile_id === form.id) return;
    if (!confirmDiscard()) return;
    applyProfile(profile);
  };

  const startNew = () => {
    if (!confirmDiscard()) return;
    setSelected(null);
    setForm(emptyForm());
    setConfigured(false);
    setDirtyState(false);
    setError(null);
    setSaved(false);
  };

  const save = async () => {
    const id = form.id.trim();
    if (!id) {
      setError(t('settings.mcp.errors.idRequired'));
      return;
    }
    const connectTimeout = positiveInteger(form.connectTimeoutSecs);
    const requestTimeout = form.requestTimeoutSecs.trim()
      ? positiveInteger(form.requestTimeoutSecs)
      : null;
    const maxLines = positiveInteger(form.maxOutputLines);
    const maxBytes = positiveInteger(form.maxOutputBytes);
    if (
      connectTimeout === null ||
      (form.requestTimeoutSecs.trim() && requestTimeout === null) ||
      maxLines === null ||
      maxBytes === null
    ) {
      setError(t('settings.mcp.errors.numbers'));
      return;
    }

    let body: PutMcpProfileRequest;
    if (form.transportType === 'http') {
      const url = form.url.trim();
      if (!url) {
        setError(t('settings.mcp.errors.urlRequired'));
        return;
      }
      const headers = parsePairs(form.headers, ':');
      if ('errorLine' in headers) {
        setError(
          t('settings.mcp.errors.headerLine', { line: headers.errorLine }),
        );
        return;
      }
      if (
        selected?.transport.type === 'http' &&
        selected.transport.bearer_token_configured &&
        !form.bearerToken.trim()
      ) {
        setError(t('settings.mcp.errors.bearerRequired'));
        return;
      }
      const missingHeader = redactedKeysMissing(
        selected?.transport.type === 'http'
          ? selected.transport.header_names
          : [],
        headers.values,
        true,
      );
      if (missingHeader) {
        setError(
          t('settings.mcp.errors.secretNameRequired', {
            name: missingHeader,
          }),
        );
        return;
      }
      body = {
        transport: {
          type: 'http',
          url,
          bearer_token: form.bearerToken.trim() || null,
          headers: headers.values,
          allow_stateless: form.allowStateless,
          reinitialize_on_expired_session: form.reinitializeOnExpiredSession,
        },
        tool_name_prefix: form.toolNamePrefix.trim() || null,
        connect_timeout_secs: connectTimeout,
        request_timeout_secs: requestTimeout,
        max_output_lines: maxLines,
        max_output_bytes: maxBytes,
      };
    } else {
      const command = form.command.trim();
      if (!command) {
        setError(t('settings.mcp.errors.commandRequired'));
        return;
      }
      const env = parsePairs(form.env, '=');
      if ('errorLine' in env) {
        setError(t('settings.mcp.errors.envLine', { line: env.errorLine }));
        return;
      }
      const missingEnv = redactedKeysMissing(
        selected?.transport.type === 'stdio' ? selected.transport.env_keys : [],
        env.values,
        false,
      );
      if (missingEnv) {
        setError(
          t('settings.mcp.errors.secretNameRequired', { name: missingEnv }),
        );
        return;
      }
      body = {
        transport: {
          type: 'stdio',
          command,
          args: splitLines(form.args, false),
          current_dir: form.currentDir.trim() || null,
          env: env.values,
          clear_env: form.clearEnv,
        },
        tool_name_prefix: form.toolNamePrefix.trim() || null,
        connect_timeout_secs: connectTimeout,
        request_timeout_secs: requestTimeout,
        max_output_lines: maxLines,
        max_output_bytes: maxBytes,
      };
    }

    setSaving(true);
    setError(null);
    setSaved(false);
    try {
      const response = await putMcpProfile(authKey.trim(), id, body);
      if (!response.configured || response.mcp_profile === null) {
        setError(t('settings.mcp.errors.notConfigured'));
        return;
      }
      const savedProfile = response.mcp_profile;
      setProfiles((current) => upsertMcpProfile(current, savedProfile));
      applyProfile(savedProfile);
      setSaved(true);
    } catch (saveError) {
      setError(
        saveError instanceof Error ? saveError.message : String(saveError),
      );
    } finally {
      setSaving(false);
    }
  };

  const hasRedactedSecrets =
    selected?.transport.type === 'http' && form.transportType === 'http'
      ? selected.transport.bearer_token_configured ||
        selected.transport.header_names.length > 0
      : selected?.transport.type === 'stdio' && form.transportType === 'stdio'
        ? selected.transport.env_keys.length > 0
        : false;

  return (
    <div className={styles.layout}>
      <aside className={styles.sidebar}>
        <div className={styles.sidebarHeading}>
          <span>{t('settings.mcp.profiles')}</span>
          <small>{profiles.length}</small>
        </div>
        <div className={styles.list}>
          {profiles.map((profile) => (
            <button
              type="button"
              key={profile.mcp_profile_id}
              className={`${styles.item} ${
                configured && form.id === profile.mcp_profile_id
                  ? styles.itemSelected
                  : ''
              }`}
              onClick={() => selectProfile(profile)}
              aria-label={profile.mcp_profile_id}
              aria-current={
                configured && form.id === profile.mcp_profile_id
                  ? 'true'
                  : undefined
              }
            >
              <span className={styles.itemCopy}>
                <strong>{profile.mcp_profile_id}</strong>
                <small>
                  {profile.transport.type} · {profile.tool_name_prefix}
                </small>
              </span>
            </button>
          ))}
          {!loading && profiles.length === 0 && (
            <p className={styles.empty}>{t('settings.mcp.noProfiles')}</p>
          )}
        </div>
        <button type="button" className={styles.addButton} onClick={startNew}>
          <PlusIcon />
          {t('settings.mcp.add')}
        </button>
      </aside>

      <main className={styles.editor}>
        <div className={styles.editorHeader}>
          <div>
            <p>{t('settings.mcp.profile')}</p>
            <h3>{configured ? form.id : t('settings.mcp.newProfileTitle')}</h3>
          </div>
          <div className={styles.headerActions}>
            {selected && (
              <span className={styles.revision}>rev {selected.revision}</span>
            )}
            <button
              type="button"
              className={styles.saveButton}
              onClick={() => void save()}
              disabled={
                saving || loading || !authKey.trim() || (configured && !dirty)
              }
            >
              {saving ? t('settings.saving') : t('settings.save')}
            </button>
          </div>
        </div>

        <div className={styles.body}>
          {!configured && (
            <label className={styles.field}>
              <span>{t('settings.mcp.id')}</span>
              <input
                value={form.id}
                placeholder={t('settings.mcp.idPlaceholder')}
                onChange={(event) => update('id', event.target.value)}
              />
            </label>
          )}

          <div className={styles.transportTabs} role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={form.transportType === 'http'}
              onClick={() => update('transportType', 'http')}
            >
              {t('settings.mcp.http')}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={form.transportType === 'stdio'}
              onClick={() => update('transportType', 'stdio')}
            >
              {t('settings.mcp.stdio')}
            </button>
          </div>

          {form.transportType === 'http' ? (
            <HttpFields form={form} update={update} />
          ) : (
            <StdioFields form={form} update={update} />
          )}

          {configured && dirty && hasRedactedSecrets && (
            <div className={styles.warning}>
              {t('settings.mcp.secretUpdateWarning')}
            </div>
          )}

          <section className={styles.section}>
            <div className={styles.sectionHeader}>
              <p className={styles.sectionTitle}>
                {t('settings.mcp.toolSettings')}
              </p>
              <p className={styles.sectionCopy}>
                {t('settings.mcp.toolSettingsCopy')}
              </p>
            </div>
            <label className={styles.field}>
              <span>{t('settings.mcp.toolPrefix')}</span>
              <input
                value={form.toolNamePrefix}
                placeholder={form.id || t('settings.mcp.toolPrefixPlaceholder')}
                onChange={(event) =>
                  update('toolNamePrefix', event.target.value)
                }
              />
              <small>{t('settings.mcp.toolPrefixHint')}</small>
            </label>
            <div className={styles.twoColumns}>
              <label className={styles.field}>
                <span>{t('settings.mcp.connectTimeout')}</span>
                <input
                  type="number"
                  min="1"
                  value={form.connectTimeoutSecs}
                  onChange={(event) =>
                    update('connectTimeoutSecs', event.target.value)
                  }
                />
              </label>
              <label className={styles.field}>
                <span>{t('settings.mcp.requestTimeout')}</span>
                <input
                  type="number"
                  min="1"
                  value={form.requestTimeoutSecs}
                  placeholder={t('settings.mcp.noTimeout')}
                  onChange={(event) =>
                    update('requestTimeoutSecs', event.target.value)
                  }
                />
              </label>
            </div>
            <div className={styles.twoColumns}>
              <label className={styles.field}>
                <span>{t('settings.mcp.maxOutputLines')}</span>
                <input
                  type="number"
                  min="1"
                  value={form.maxOutputLines}
                  onChange={(event) =>
                    update('maxOutputLines', event.target.value)
                  }
                />
              </label>
              <label className={styles.field}>
                <span>{t('settings.mcp.maxOutputBytes')}</span>
                <input
                  type="number"
                  min="1"
                  value={form.maxOutputBytes}
                  onChange={(event) =>
                    update('maxOutputBytes', event.target.value)
                  }
                />
              </label>
            </div>
          </section>

          {error && (
            <div className={styles.error} role="alert">
              {error}
            </div>
          )}
          {saved && (
            <div className={styles.success}>{t('settings.mcp.saved')}</div>
          )}
        </div>
      </main>
    </div>
  );
}

function HttpFields({
  form,
  update,
}: {
  form: McpProfileForm;
  update: <K extends keyof McpProfileForm>(
    key: K,
    value: McpProfileForm[K],
  ) => void;
}) {
  const { t } = useI18n();
  return (
    <section className={styles.section}>
      <div className={styles.sectionHeader}>
        <p className={styles.sectionTitle}>{t('settings.mcp.httpSettings')}</p>
        <p className={styles.sectionCopy}>
          {t('settings.mcp.httpSettingsCopy')}
        </p>
      </div>
      <label className={styles.field}>
        <span>{t('settings.mcp.url')}</span>
        <input
          value={form.url}
          placeholder="https://mcp.example.com/mcp"
          onChange={(event) => update('url', event.target.value)}
        />
      </label>
      <label className={styles.field}>
        <span>{t('settings.mcp.bearerToken')}</span>
        <input
          type="password"
          value={form.bearerToken}
          placeholder={t('settings.mcp.secretPlaceholder')}
          onChange={(event) => update('bearerToken', event.target.value)}
        />
      </label>
      <label className={styles.field}>
        <span>{t('settings.mcp.headers')}</span>
        <textarea
          value={form.headers}
          placeholder={t('settings.mcp.headersPlaceholder')}
          onChange={(event) => update('headers', event.target.value)}
        />
        <small>{t('settings.mcp.headersHint')}</small>
      </label>
      <label className={styles.checkboxRow}>
        <input
          type="checkbox"
          checked={form.allowStateless}
          onChange={(event) => update('allowStateless', event.target.checked)}
        />
        {t('settings.mcp.allowStateless')}
      </label>
      <label className={styles.checkboxRow}>
        <input
          type="checkbox"
          checked={form.reinitializeOnExpiredSession}
          onChange={(event) =>
            update('reinitializeOnExpiredSession', event.target.checked)
          }
        />
        {t('settings.mcp.reinitialize')}
      </label>
    </section>
  );
}

function StdioFields({
  form,
  update,
}: {
  form: McpProfileForm;
  update: <K extends keyof McpProfileForm>(
    key: K,
    value: McpProfileForm[K],
  ) => void;
}) {
  const { t } = useI18n();
  return (
    <section className={styles.section}>
      <div className={styles.sectionHeader}>
        <p className={styles.sectionTitle}>{t('settings.mcp.stdioSettings')}</p>
        <p className={styles.sectionCopy}>
          {t('settings.mcp.stdioSettingsCopy')}
        </p>
      </div>
      <label className={styles.field}>
        <span>{t('settings.mcp.command')}</span>
        <input
          value={form.command}
          placeholder="npx"
          onChange={(event) => update('command', event.target.value)}
        />
      </label>
      <label className={styles.field}>
        <span>{t('settings.mcp.args')}</span>
        <textarea
          value={form.args}
          placeholder={t('settings.mcp.argsPlaceholder')}
          onChange={(event) => update('args', event.target.value)}
        />
        <small>{t('settings.mcp.onePerLine')}</small>
      </label>
      <label className={styles.field}>
        <span>{t('settings.mcp.currentDir')}</span>
        <input
          value={form.currentDir}
          placeholder={t('settings.mcp.currentDirPlaceholder')}
          onChange={(event) => update('currentDir', event.target.value)}
        />
      </label>
      <label className={styles.field}>
        <span>{t('settings.mcp.environment')}</span>
        <textarea
          value={form.env}
          placeholder={t('settings.mcp.envPlaceholder')}
          onChange={(event) => update('env', event.target.value)}
        />
        <small>{t('settings.mcp.envHint')}</small>
      </label>
      <label className={styles.checkboxRow}>
        <input
          type="checkbox"
          checked={form.clearEnv}
          onChange={(event) => update('clearEnv', event.target.checked)}
        />
        {t('settings.mcp.clearEnv')}
      </label>
    </section>
  );
}

function positiveInteger(value: string): number | null {
  const normalized = value.trim();
  if (!/^\d+$/.test(normalized)) return null;
  const parsed = Number(normalized);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : null;
}

function splitLines(value: string, trim = true): string[] {
  return value
    .split(/\r?\n/)
    .map((line) => (trim ? line.trim() : line))
    .filter((line) => line.length > 0);
}

function parsePairs(
  value: string,
  delimiter: ':' | '=',
): { values: Record<string, string> } | { errorLine: number } {
  const values: Record<string, string> = {};
  const lines = value.split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (!line.trim()) continue;
    const delimiterIndex = line.indexOf(delimiter);
    if (delimiterIndex <= 0) return { errorLine: index + 1 };
    const name = line.slice(0, delimiterIndex).trim();
    const secret = line.slice(delimiterIndex + 1).trim();
    if (!name || !secret) return { errorLine: index + 1 };
    values[name] = secret;
  }
  return { values };
}

function redactedKeysMissing(
  required: readonly string[],
  provided: Record<string, string>,
  caseInsensitive: boolean,
): string | null {
  const normalize = (name: string) =>
    caseInsensitive ? name.toLowerCase() : name;
  const providedNames = new Set(Object.keys(provided).map(normalize));
  return required.find((name) => !providedNames.has(normalize(name))) ?? null;
}

function upsertMcpProfile(
  profiles: readonly PublicMcpProfile[],
  saved: PublicMcpProfile,
): PublicMcpProfile[] {
  const found = profiles.some(
    (profile) => profile.mcp_profile_id === saved.mcp_profile_id,
  );
  if (!found) return [...profiles, saved];
  return profiles.map((profile) =>
    profile.mcp_profile_id === saved.mcp_profile_id ? saved : profile,
  );
}
