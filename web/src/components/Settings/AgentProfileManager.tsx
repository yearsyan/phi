import { useCallback, useEffect, useRef, useState } from 'react';
import {
  listAgentProfiles,
  listMcpProfiles,
  putAgentProfile,
} from '../../api/http.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type {
  CapabilityMode,
  PublicAgentProfile,
  PublicMcpProfile,
  PutAgentProfileRequest,
  ReasoningEffort,
} from '../../types/wire.ts';
import { PlusIcon } from '../common/Icons.tsx';
import styles from './ProfileManager.module.css';

const EFFORTS: ReasoningEffort[] = [
  'none',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
];

interface AgentProfileManagerProps {
  authKey: string;
  onDirtyChange: (dirty: boolean) => void;
}

interface AgentProfileForm {
  id: string;
  promptMode: 'extend' | 'full';
  promptText: string;
  toolAllowEnabled: boolean;
  toolAllow: string;
  toolDeny: string;
  skillAllowEnabled: boolean;
  skillAllow: string;
  skillDeny: string;
  mcpProfileIds: string[];
  capabilityMode: CapabilityMode;
  model: string;
  reasoningEffort: ReasoningEffort | '';
}

function emptyForm(): AgentProfileForm {
  return {
    id: '',
    promptMode: 'extend',
    promptText: '',
    toolAllowEnabled: false,
    toolAllow: '',
    toolDeny: '',
    skillAllowEnabled: false,
    skillAllow: '',
    skillDeny: '',
    mcpProfileIds: [],
    capabilityMode: 'full_access',
    model: '',
    reasoningEffort: '',
  };
}

function fromProfile(profile: PublicAgentProfile): AgentProfileForm {
  return {
    id: profile.agent_profile_id,
    promptMode: profile.prompt.mode,
    promptText: profile.prompt.text,
    toolAllowEnabled: profile.tools.allow != null,
    toolAllow: profile.tools.allow?.join('\n') ?? '',
    toolDeny: profile.tools.deny?.join('\n') ?? '',
    skillAllowEnabled: profile.skills.allow != null,
    skillAllow: profile.skills.allow?.join('\n') ?? '',
    skillDeny: profile.skills.deny?.join('\n') ?? '',
    mcpProfileIds: profile.mcp_profile_ids ?? [],
    capabilityMode: profile.initial_capability_mode,
    model: profile.model ?? '',
    reasoningEffort: profile.reasoning_effort ?? '',
  };
}

export function AgentProfileManager({
  authKey,
  onDirtyChange,
}: AgentProfileManagerProps) {
  const { t } = useI18n();
  const [profiles, setProfiles] = useState<PublicAgentProfile[]>([]);
  const [mcpProfiles, setMcpProfiles] = useState<PublicMcpProfile[]>([]);
  const [form, setForm] = useState<AgentProfileForm>(emptyForm);
  const [configured, setConfigured] = useState(false);
  const [revision, setRevision] = useState<number | null>(null);
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
    (profile: PublicAgentProfile) => {
      setForm(fromProfile(profile));
      setConfigured(true);
      setRevision(profile.revision);
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
      const [agentResponse, mcpResponse] = await Promise.all([
        listAgentProfiles(authKey.trim()),
        listMcpProfiles(authKey.trim()),
      ]);
      if (loadRevision.current !== currentRevision) return;
      setProfiles(agentResponse.agent_profiles);
      setMcpProfiles(mcpResponse.mcp_profiles);
      const selected = agentResponse.agent_profiles[0];
      if (selected) {
        applyProfile(selected);
      } else {
        setForm(emptyForm());
        setConfigured(false);
        setRevision(null);
        setDirtyState(false);
      }
    } catch (loadError) {
      if (loadRevision.current !== currentRevision) return;
      setProfiles([]);
      setMcpProfiles([]);
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

  const update = <K extends keyof AgentProfileForm>(
    key: K,
    value: AgentProfileForm[K],
  ) => {
    setForm((current) => ({ ...current, [key]: value }));
    setDirtyState(true);
    setSaved(false);
  };

  const confirmDiscard = () =>
    !dirty || window.confirm(t('settings.agent.discardChanges'));

  const selectProfile = (profile: PublicAgentProfile) => {
    if (configured && profile.agent_profile_id === form.id) return;
    if (!confirmDiscard()) return;
    applyProfile(profile);
  };

  const startNew = () => {
    if (!confirmDiscard()) return;
    setForm(emptyForm());
    setConfigured(false);
    setRevision(null);
    setDirtyState(false);
    setError(null);
    setSaved(false);
  };

  const toggleMcp = (id: string) => {
    update(
      'mcpProfileIds',
      form.mcpProfileIds.includes(id)
        ? form.mcpProfileIds.filter((candidate) => candidate !== id)
        : [...form.mcpProfileIds, id],
    );
  };

  const save = async () => {
    const id = form.id.trim();
    if (!id) {
      setError(t('settings.agent.errors.idRequired'));
      return;
    }
    const body: PutAgentProfileRequest = {
      prompt: { mode: form.promptMode, text: form.promptText },
      tools: {
        allow: form.toolAllowEnabled ? splitNames(form.toolAllow) : null,
        deny: splitNames(form.toolDeny),
      },
      skills: {
        allow: form.skillAllowEnabled ? splitNames(form.skillAllow) : null,
        deny: splitNames(form.skillDeny),
      },
      mcp_profile_ids: form.mcpProfileIds,
      initial_capability_mode: form.capabilityMode,
      model: form.model.trim() || null,
      reasoning_effort: form.reasoningEffort || null,
    };

    setSaving(true);
    setError(null);
    setSaved(false);
    try {
      const response = await putAgentProfile(authKey.trim(), id, body);
      if (!response.configured || response.agent_profile === null) {
        setError(t('settings.agent.errors.notConfigured'));
        return;
      }
      const savedProfile = response.agent_profile;
      setProfiles((current) => upsertAgentProfile(current, savedProfile));
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

  return (
    <div className={styles.layout}>
      <aside className={styles.sidebar}>
        <div className={styles.sidebarHeading}>
          <span>{t('settings.agent.profiles')}</span>
          <small>{profiles.length}</small>
        </div>
        <div className={styles.list}>
          {profiles.map((profile) => (
            <button
              type="button"
              key={profile.agent_profile_id}
              className={`${styles.item} ${
                configured && form.id === profile.agent_profile_id
                  ? styles.itemSelected
                  : ''
              }`}
              onClick={() => selectProfile(profile)}
              aria-label={profile.agent_profile_id}
              aria-current={
                configured && form.id === profile.agent_profile_id
                  ? 'true'
                  : undefined
              }
            >
              <span className={styles.itemCopy}>
                <strong>{profile.agent_profile_id}</strong>
                <small>
                  {profile.mcp_profile_ids?.length ?? 0}{' '}
                  {t('settings.agent.mcpCount')}
                </small>
              </span>
            </button>
          ))}
          {!loading && profiles.length === 0 && (
            <p className={styles.empty}>{t('settings.agent.noProfiles')}</p>
          )}
        </div>
        <button type="button" className={styles.addButton} onClick={startNew}>
          <PlusIcon />
          {t('settings.agent.add')}
        </button>
      </aside>

      <main className={styles.editor}>
        <div className={styles.editorHeader}>
          <div>
            <p>{t('settings.agent.profile')}</p>
            <h3>
              {configured ? form.id : t('settings.agent.newProfileTitle')}
            </h3>
          </div>
          <div className={styles.headerActions}>
            {revision !== null && (
              <span className={styles.revision}>rev {revision}</span>
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
              <span>{t('settings.agent.id')}</span>
              <input
                value={form.id}
                placeholder={t('settings.agent.idPlaceholder')}
                onChange={(event) => update('id', event.target.value)}
              />
            </label>
          )}

          <div className={styles.twoColumns}>
            <label className={styles.field}>
              <span>{t('settings.agent.promptMode')}</span>
              <select
                value={form.promptMode}
                onChange={(event) =>
                  update('promptMode', event.target.value as 'extend' | 'full')
                }
              >
                <option value="extend">
                  {t('settings.agent.promptExtend')}
                </option>
                <option value="full">{t('settings.agent.promptFull')}</option>
              </select>
            </label>
            <label className={styles.field}>
              <span>{t('settings.agent.capability')}</span>
              <select
                value={form.capabilityMode}
                onChange={(event) =>
                  update('capabilityMode', event.target.value as CapabilityMode)
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
          </div>

          <label className={`${styles.field} ${styles.promptField}`}>
            <span>{t('settings.agent.prompt')}</span>
            <textarea
              value={form.promptText}
              placeholder={t('settings.agent.promptPlaceholder')}
              onChange={(event) => update('promptText', event.target.value)}
            />
          </label>

          <PolicyEditor
            title={t('settings.agent.toolPolicy')}
            copy={t('settings.agent.toolPolicyCopy')}
            allowEnabled={form.toolAllowEnabled}
            allow={form.toolAllow}
            deny={form.toolDeny}
            onAllowEnabled={(value) => update('toolAllowEnabled', value)}
            onAllow={(value) => update('toolAllow', value)}
            onDeny={(value) => update('toolDeny', value)}
          />

          <PolicyEditor
            title={t('settings.agent.skillPolicy')}
            copy={t('settings.agent.skillPolicyCopy')}
            allowEnabled={form.skillAllowEnabled}
            allow={form.skillAllow}
            deny={form.skillDeny}
            onAllowEnabled={(value) => update('skillAllowEnabled', value)}
            onAllow={(value) => update('skillAllow', value)}
            onDeny={(value) => update('skillDeny', value)}
          />

          <section className={styles.section}>
            <div className={styles.sectionHeader}>
              <p className={styles.sectionTitle}>
                {t('settings.agent.mcpProfiles')}
              </p>
              <p className={styles.sectionCopy}>
                {t('settings.agent.mcpProfilesCopy')}
              </p>
            </div>
            {mcpProfiles.length > 0 ? (
              <div className={styles.choiceGrid}>
                {mcpProfiles.map((profile) => (
                  <label className={styles.choice} key={profile.mcp_profile_id}>
                    <input
                      type="checkbox"
                      checked={form.mcpProfileIds.includes(
                        profile.mcp_profile_id,
                      )}
                      onChange={() => toggleMcp(profile.mcp_profile_id)}
                    />
                    <span>
                      <strong>{profile.mcp_profile_id}</strong>
                      <small>
                        {profile.transport.type} · {profile.tool_name_prefix}
                      </small>
                    </span>
                  </label>
                ))}
              </div>
            ) : (
              <p className={styles.sectionCopy}>
                {t('settings.agent.noMcpProfiles')}
              </p>
            )}
            {form.mcpProfileIds.length > 0 &&
              form.capabilityMode !== 'full_access' && (
                <div className={styles.warning}>
                  {t('settings.agent.mcpCapabilityWarning')}
                </div>
              )}
          </section>

          <div className={styles.twoColumns}>
            <label className={styles.field}>
              <span>{t('settings.agent.model')}</span>
              <input
                value={form.model}
                placeholder={t('settings.agent.modelPlaceholder')}
                onChange={(event) => update('model', event.target.value)}
              />
            </label>
            <label className={styles.field}>
              <span>{t('settings.agent.reasoning')}</span>
              <select
                value={form.reasoningEffort}
                onChange={(event) =>
                  update(
                    'reasoningEffort',
                    event.target.value as ReasoningEffort | '',
                  )
                }
              >
                <option value="">{t('settings.agent.providerDefault')}</option>
                {EFFORTS.map((effort) => (
                  <option value={effort} key={effort}>
                    {effort}
                  </option>
                ))}
              </select>
            </label>
          </div>

          {error && (
            <div className={styles.error} role="alert">
              {error}
            </div>
          )}
          {saved && (
            <div className={styles.success}>{t('settings.agent.saved')}</div>
          )}
        </div>
      </main>
    </div>
  );
}

function PolicyEditor({
  title,
  copy,
  allowEnabled,
  allow,
  deny,
  onAllowEnabled,
  onAllow,
  onDeny,
}: {
  title: string;
  copy: string;
  allowEnabled: boolean;
  allow: string;
  deny: string;
  onAllowEnabled: (value: boolean) => void;
  onAllow: (value: string) => void;
  onDeny: (value: string) => void;
}) {
  const { t } = useI18n();
  return (
    <section className={styles.section}>
      <div className={styles.sectionHeader}>
        <p className={styles.sectionTitle}>{title}</p>
        <p className={styles.sectionCopy}>{copy}</p>
      </div>
      <label className={styles.checkboxRow}>
        <input
          type="checkbox"
          checked={allowEnabled}
          onChange={(event) => onAllowEnabled(event.target.checked)}
        />
        {t('settings.agent.useAllowList')}
      </label>
      <div className={styles.twoColumns}>
        <label className={styles.field}>
          <span>{t('settings.agent.allowNames')}</span>
          <textarea
            value={allow}
            disabled={!allowEnabled}
            placeholder={t('settings.agent.namesPlaceholder')}
            onChange={(event) => onAllow(event.target.value)}
          />
        </label>
        <label className={styles.field}>
          <span>{t('settings.agent.denyNames')}</span>
          <textarea
            value={deny}
            placeholder={t('settings.agent.namesPlaceholder')}
            onChange={(event) => onDeny(event.target.value)}
          />
        </label>
      </div>
    </section>
  );
}

function splitNames(value: string): string[] {
  return value
    .split(/\r?\n/)
    .map((name) => name.trim())
    .filter(Boolean);
}

function upsertAgentProfile(
  profiles: readonly PublicAgentProfile[],
  saved: PublicAgentProfile,
): PublicAgentProfile[] {
  const found = profiles.some(
    (profile) => profile.agent_profile_id === saved.agent_profile_id,
  );
  if (!found) return [...profiles, saved];
  return profiles.map((profile) =>
    profile.agent_profile_id === saved.agent_profile_id ? saved : profile,
  );
}
