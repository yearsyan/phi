import { useCallback, useEffect, useRef, useState } from 'react';
import {
  listOutputChannels,
  putOutputChannel,
  testOutputChannel,
} from '../../api/http.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { PublicOutputChannel } from '../../types/wire.ts';
import { PlusIcon } from '../common/Icons.tsx';
import styles from './ProfileManager.module.css';

interface OutputChannelManagerProps {
  authKey: string;
  onDirtyChange: (dirty: boolean) => void;
}

interface OutputChannelForm {
  id: string;
  botToken: string;
  chatId: string;
}

function emptyForm(): OutputChannelForm {
  return { id: '', botToken: '', chatId: '' };
}

function fromChannel(channel: PublicOutputChannel): OutputChannelForm {
  return {
    id: channel.output_channel_id,
    botToken: '',
    chatId: channel.chat_id,
  };
}

export function OutputChannelManager({
  authKey,
  onDirtyChange,
}: OutputChannelManagerProps) {
  const { t } = useI18n();
  const [channels, setChannels] = useState<PublicOutputChannel[]>([]);
  const [selected, setSelected] = useState<PublicOutputChannel | null>(null);
  const [form, setForm] = useState<OutputChannelForm>(emptyForm);
  const [configured, setConfigured] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const loadRevision = useRef(0);

  const setDirtyState = useCallback(
    (value: boolean) => {
      setDirty(value);
      onDirtyChange(value);
    },
    [onDirtyChange],
  );

  const applyChannel = useCallback(
    (channel: PublicOutputChannel) => {
      setSelected(channel);
      setForm(fromChannel(channel));
      setConfigured(true);
      setDirtyState(false);
      setError(null);
      setStatus(null);
    },
    [setDirtyState],
  );

  const load = useCallback(async () => {
    if (!authKey.trim()) return;
    const revision = ++loadRevision.current;
    setLoading(true);
    setError(null);
    try {
      const response = await listOutputChannels(authKey.trim());
      if (revision !== loadRevision.current) return;
      setChannels(response.output_channels);
      const first = response.output_channels[0];
      if (first) {
        applyChannel(first);
      } else {
        setSelected(null);
        setForm(emptyForm());
        setConfigured(false);
        setDirtyState(false);
      }
    } catch (loadError) {
      if (revision !== loadRevision.current) return;
      setChannels([]);
      setError(
        loadError instanceof Error ? loadError.message : String(loadError),
      );
    } finally {
      if (revision === loadRevision.current) setLoading(false);
    }
  }, [applyChannel, authKey, setDirtyState]);

  useEffect(() => {
    void load();
  }, [load]);

  const update = <K extends keyof OutputChannelForm>(
    key: K,
    value: OutputChannelForm[K],
  ) => {
    setForm((current) => ({ ...current, [key]: value }));
    setDirtyState(true);
    setError(null);
    setStatus(null);
  };

  const confirmDiscard = () =>
    !dirty || window.confirm(t('settings.channels.discardChanges'));

  const selectChannel = (channel: PublicOutputChannel) => {
    if (configured && channel.output_channel_id === form.id) return;
    if (!confirmDiscard()) return;
    applyChannel(channel);
  };

  const startNew = () => {
    if (!confirmDiscard()) return;
    setSelected(null);
    setForm(emptyForm());
    setConfigured(false);
    setDirtyState(false);
    setError(null);
    setStatus(null);
  };

  const save = async () => {
    const id = form.id.trim();
    if (!id) {
      setError(t('settings.channels.errors.idRequired'));
      return;
    }
    const botToken = form.botToken.trim();
    if (!botToken) {
      setError(t('settings.channels.errors.tokenRequired'));
      return;
    }
    const chatId = form.chatId.trim();
    if (!chatId) {
      setError(t('settings.channels.errors.chatIdRequired'));
      return;
    }
    setSaving(true);
    setError(null);
    setStatus(null);
    try {
      const response = await putOutputChannel(authKey.trim(), id, {
        type: 'telegram',
        bot_token: botToken,
        chat_id: chatId,
      });
      if (!response.configured || response.output_channel === null) {
        setError(t('settings.channels.errors.notConfigured'));
        return;
      }
      const savedChannel = response.output_channel;
      setChannels((current) => upsertChannel(current, savedChannel));
      applyChannel(savedChannel);
      setStatus(t('settings.channels.saved'));
    } catch (saveError) {
      setError(
        saveError instanceof Error ? saveError.message : String(saveError),
      );
    } finally {
      setSaving(false);
    }
  };

  const test = async () => {
    if (!selected || dirty) return;
    setTesting(true);
    setError(null);
    setStatus(null);
    try {
      await testOutputChannel(authKey.trim(), selected.output_channel_id);
      setStatus(t('settings.channels.testSent'));
    } catch (testError) {
      setError(
        testError instanceof Error ? testError.message : String(testError),
      );
    } finally {
      setTesting(false);
    }
  };

  return (
    <div className={styles.layout}>
      <aside className={styles.sidebar}>
        <div className={styles.sidebarHeading}>
          <span>{t('settings.channels.list')}</span>
          <small>{channels.length}</small>
        </div>
        <div className={styles.list}>
          {channels.map((channel) => (
            <button
              type="button"
              key={channel.output_channel_id}
              className={`${styles.item} ${
                configured && form.id === channel.output_channel_id
                  ? styles.itemSelected
                  : ''
              }`}
              onClick={() => selectChannel(channel)}
              aria-label={channel.output_channel_id}
              aria-current={
                configured && form.id === channel.output_channel_id
                  ? 'true'
                  : undefined
              }
            >
              <span className={styles.itemCopy}>
                <strong>{channel.output_channel_id}</strong>
                <small>Telegram · {channel.chat_id}</small>
              </span>
            </button>
          ))}
          {!loading && channels.length === 0 && (
            <p className={styles.empty}>{t('settings.channels.empty')}</p>
          )}
        </div>
        <button type="button" className={styles.addButton} onClick={startNew}>
          <PlusIcon />
          {t('settings.channels.add')}
        </button>
      </aside>

      <main className={styles.editor}>
        <div className={styles.editorHeader}>
          <div>
            <p>{t('settings.channels.channel')}</p>
            <h3>{configured ? form.id : t('settings.channels.newChannel')}</h3>
          </div>
          <div className={styles.headerActions}>
            {selected && (
              <span className={styles.revision}>rev {selected.revision}</span>
            )}
            {selected && (
              <button
                type="button"
                className={styles.saveButton}
                onClick={() => void test()}
                disabled={testing || saving || dirty || !authKey.trim()}
              >
                {testing
                  ? t('settings.channels.testing')
                  : t('settings.channels.test')}
              </button>
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
              <span>{t('settings.channels.id')}</span>
              <input
                value={form.id}
                placeholder={t('settings.channels.idPlaceholder')}
                onChange={(event) => update('id', event.target.value)}
              />
            </label>
          )}

          <section className={styles.section}>
            <div className={styles.sectionHeader}>
              <p className={styles.sectionTitle}>
                {t('settings.channels.telegram')}
              </p>
              <p className={styles.sectionCopy}>
                {t('settings.channels.telegramCopy')}
              </p>
            </div>
            <label className={styles.field}>
              <span>{t('settings.channels.botToken')}</span>
              <input
                type="password"
                autoComplete="off"
                value={form.botToken}
                placeholder={t('settings.channels.secretPlaceholder')}
                onChange={(event) => update('botToken', event.target.value)}
              />
              <small>{t('settings.channels.botTokenHint')}</small>
            </label>
            <label className={styles.field}>
              <span>{t('settings.channels.chatId')}</span>
              <input
                value={form.chatId}
                placeholder="-1001234567890"
                onChange={(event) => update('chatId', event.target.value)}
              />
              <small>{t('settings.channels.chatIdHint')}</small>
            </label>
          </section>

          {configured && dirty && selected?.bot_token_configured && (
            <div className={styles.warning}>
              {t('settings.channels.secretUpdateWarning')}
            </div>
          )}
          {error && (
            <div className={styles.error} role="alert">
              {error}
            </div>
          )}
          {status && <div className={styles.success}>{status}</div>}
        </div>
      </main>
    </div>
  );
}

function upsertChannel(
  channels: PublicOutputChannel[],
  replacement: PublicOutputChannel,
): PublicOutputChannel[] {
  const next = channels.filter(
    (channel) => channel.output_channel_id !== replacement.output_channel_id,
  );
  next.push(replacement);
  next.sort((left, right) =>
    left.output_channel_id.localeCompare(right.output_channel_id),
  );
  return next;
}
