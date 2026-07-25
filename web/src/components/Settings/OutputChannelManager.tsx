import { useCallback, useEffect, useRef, useState } from 'react';
import {
  listBotAccounts,
  listOutputChannels,
  putBotAccount,
  putOutputChannel,
  testOutputChannel,
} from '../../api/http.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type {
  PublicBotAccount,
  PublicOutputChannel,
} from '../../types/wire.ts';
import { PlusIcon } from '../common/Icons.tsx';
import styles from './ProfileManager.module.css';

interface OutputChannelManagerProps {
  authKey: string;
  onDirtyChange: (dirty: boolean) => void;
}

type EditorMode = 'bots' | 'targets';

interface BotAccountForm {
  id: string;
  botToken: string;
}

interface RecipientTargetForm {
  id: string;
  botAccountId: string;
  chatId: string;
}

function emptyBotForm(): BotAccountForm {
  return { id: '', botToken: '' };
}

function emptyTargetForm(botAccountId = ''): RecipientTargetForm {
  return { id: '', botAccountId, chatId: '' };
}

function fromBotAccount(account: PublicBotAccount): BotAccountForm {
  return { id: account.bot_account_id, botToken: '' };
}

function fromTarget(target: PublicOutputChannel): RecipientTargetForm {
  return {
    id: target.output_channel_id,
    botAccountId: target.bot_account_id,
    chatId: target.chat_id,
  };
}

export function OutputChannelManager({
  authKey,
  onDirtyChange,
}: OutputChannelManagerProps) {
  const { t } = useI18n();
  const [mode, setMode] = useState<EditorMode>('bots');
  const [botAccounts, setBotAccounts] = useState<PublicBotAccount[]>([]);
  const [targets, setTargets] = useState<PublicOutputChannel[]>([]);
  const [selectedBot, setSelectedBot] = useState<PublicBotAccount | null>(null);
  const [selectedTarget, setSelectedTarget] =
    useState<PublicOutputChannel | null>(null);
  const [botForm, setBotForm] = useState<BotAccountForm>(emptyBotForm);
  const [targetForm, setTargetForm] =
    useState<RecipientTargetForm>(emptyTargetForm);
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

  const clearFeedback = () => {
    setError(null);
    setStatus(null);
  };

  const applyBot = useCallback(
    (account: PublicBotAccount) => {
      setMode('bots');
      setSelectedBot(account);
      setBotForm(fromBotAccount(account));
      setDirtyState(false);
      setError(null);
      setStatus(null);
    },
    [setDirtyState],
  );

  const applyTarget = useCallback(
    (target: PublicOutputChannel) => {
      setMode('targets');
      setSelectedTarget(target);
      setTargetForm(fromTarget(target));
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
      const [botResponse, targetResponse] = await Promise.all([
        listBotAccounts(authKey.trim()),
        listOutputChannels(authKey.trim()),
      ]);
      if (revision !== loadRevision.current) return;
      setBotAccounts(botResponse.bot_accounts);
      setTargets(targetResponse.output_channels);
      const firstBot = botResponse.bot_accounts[0];
      if (firstBot) {
        applyBot(firstBot);
      } else {
        setMode('bots');
        setSelectedBot(null);
        setBotForm(emptyBotForm());
        setTargetForm(emptyTargetForm());
        setDirtyState(false);
      }
    } catch (loadError) {
      if (revision !== loadRevision.current) return;
      setBotAccounts([]);
      setTargets([]);
      setError(
        loadError instanceof Error ? loadError.message : String(loadError),
      );
    } finally {
      if (revision === loadRevision.current) setLoading(false);
    }
  }, [applyBot, authKey, setDirtyState]);

  useEffect(() => {
    void load();
  }, [load]);

  const confirmDiscard = () =>
    !dirty || window.confirm(t('settings.channels.discardChanges'));

  const switchMode = (nextMode: EditorMode) => {
    if (nextMode === mode || !confirmDiscard()) return;
    if (nextMode === 'bots') {
      const first = botAccounts[0];
      if (first) {
        applyBot(first);
      } else {
        setMode('bots');
        setSelectedBot(null);
        setBotForm(emptyBotForm());
        setDirtyState(false);
        clearFeedback();
      }
      return;
    }
    const first = targets[0];
    if (first) {
      applyTarget(first);
    } else {
      setMode('targets');
      setSelectedTarget(null);
      setTargetForm(emptyTargetForm(botAccounts[0]?.bot_account_id));
      setDirtyState(false);
      clearFeedback();
    }
  };

  const selectBot = (account: PublicBotAccount) => {
    if (
      mode === 'bots' &&
      selectedBot?.bot_account_id === account.bot_account_id
    ) {
      return;
    }
    if (!confirmDiscard()) return;
    applyBot(account);
  };

  const selectTarget = (target: PublicOutputChannel) => {
    if (
      mode === 'targets' &&
      selectedTarget?.output_channel_id === target.output_channel_id
    ) {
      return;
    }
    if (!confirmDiscard()) return;
    applyTarget(target);
  };

  const startNew = () => {
    if (!confirmDiscard()) return;
    clearFeedback();
    setDirtyState(false);
    if (mode === 'bots') {
      setSelectedBot(null);
      setBotForm(emptyBotForm());
    } else {
      setSelectedTarget(null);
      setTargetForm(emptyTargetForm(botAccounts[0]?.bot_account_id));
    }
  };

  const updateBot = <K extends keyof BotAccountForm>(
    key: K,
    value: BotAccountForm[K],
  ) => {
    setBotForm((current) => ({ ...current, [key]: value }));
    setDirtyState(true);
    clearFeedback();
  };

  const updateTarget = <K extends keyof RecipientTargetForm>(
    key: K,
    value: RecipientTargetForm[K],
  ) => {
    setTargetForm((current) => ({ ...current, [key]: value }));
    setDirtyState(true);
    clearFeedback();
  };

  const save = async () => {
    if (mode === 'bots') {
      const id = botForm.id.trim();
      if (!id) {
        setError(t('settings.channels.errors.botIdRequired'));
        return;
      }
      const botToken = botForm.botToken.trim();
      if (!botToken) {
        setError(t('settings.channels.errors.tokenRequired'));
        return;
      }
      setSaving(true);
      clearFeedback();
      try {
        const response = await putBotAccount(authKey.trim(), id, {
          type: 'telegram',
          bot_token: botToken,
        });
        if (!response.configured || response.bot_account === null) {
          setError(t('settings.channels.errors.botNotConfigured'));
          return;
        }
        const saved = response.bot_account;
        setBotAccounts((current) => upsertBotAccount(current, saved));
        applyBot(saved);
        setStatus(t('settings.channels.botSaved'));
      } catch (saveError) {
        setError(
          saveError instanceof Error ? saveError.message : String(saveError),
        );
      } finally {
        setSaving(false);
      }
      return;
    }

    const id = targetForm.id.trim();
    if (!id) {
      setError(t('settings.channels.errors.targetIdRequired'));
      return;
    }
    const botAccountId = targetForm.botAccountId.trim();
    if (!botAccountId) {
      setError(t('settings.channels.errors.botRequired'));
      return;
    }
    const chatId = targetForm.chatId.trim();
    if (!chatId) {
      setError(t('settings.channels.errors.chatIdRequired'));
      return;
    }
    setSaving(true);
    clearFeedback();
    try {
      const response = await putOutputChannel(authKey.trim(), id, {
        type: 'telegram',
        bot_account_id: botAccountId,
        chat_id: chatId,
      });
      if (!response.configured || response.output_channel === null) {
        setError(t('settings.channels.errors.targetNotConfigured'));
        return;
      }
      const saved = response.output_channel;
      setTargets((current) => upsertTarget(current, saved));
      applyTarget(saved);
      setStatus(t('settings.channels.targetSaved'));
    } catch (saveError) {
      setError(
        saveError instanceof Error ? saveError.message : String(saveError),
      );
    } finally {
      setSaving(false);
    }
  };

  const test = async () => {
    if (mode !== 'targets' || !selectedTarget || dirty) return;
    setTesting(true);
    clearFeedback();
    try {
      await testOutputChannel(authKey.trim(), selectedTarget.output_channel_id);
      setStatus(t('settings.channels.testSent'));
    } catch (testError) {
      setError(
        testError instanceof Error ? testError.message : String(testError),
      );
    } finally {
      setTesting(false);
    }
  };

  const selectedRevision =
    mode === 'bots' ? selectedBot?.revision : selectedTarget?.revision;
  const configured =
    mode === 'bots' ? selectedBot !== null : selectedTarget !== null;
  const currentItems = mode === 'bots' ? botAccounts : targets;

  return (
    <div className={styles.layout}>
      <aside className={styles.sidebar}>
        <div className={styles.transportTabs} role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={mode === 'bots'}
            onClick={() => switchMode('bots')}
          >
            {t('settings.channels.botAccountsTab')}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={mode === 'targets'}
            onClick={() => switchMode('targets')}
          >
            {t('settings.channels.targetsTab')}
          </button>
        </div>
        <div className={styles.sidebarHeading}>
          <span>
            {mode === 'bots'
              ? t('settings.channels.botAccounts')
              : t('settings.channels.targets')}
          </span>
          <small>{currentItems.length}</small>
        </div>
        <div className={styles.list}>
          {mode === 'bots'
            ? botAccounts.map((account) => (
                <button
                  type="button"
                  key={account.bot_account_id}
                  className={`${styles.item} ${
                    selectedBot?.bot_account_id === account.bot_account_id
                      ? styles.itemSelected
                      : ''
                  }`}
                  onClick={() => selectBot(account)}
                  aria-label={account.bot_account_id}
                >
                  <span className={styles.itemCopy}>
                    <strong>{account.bot_account_id}</strong>
                    <small>Telegram bot</small>
                  </span>
                </button>
              ))
            : targets.map((target) => (
                <button
                  type="button"
                  key={target.output_channel_id}
                  className={`${styles.item} ${
                    selectedTarget?.output_channel_id ===
                    target.output_channel_id
                      ? styles.itemSelected
                      : ''
                  }`}
                  onClick={() => selectTarget(target)}
                  aria-label={target.output_channel_id}
                >
                  <span className={styles.itemCopy}>
                    <strong>{target.output_channel_id}</strong>
                    <small>
                      {target.bot_account_id} · {target.chat_id}
                    </small>
                  </span>
                </button>
              ))}
          {!loading && currentItems.length === 0 && (
            <p className={styles.empty}>
              {mode === 'bots'
                ? t('settings.channels.botsEmpty')
                : t('settings.channels.targetsEmpty')}
            </p>
          )}
        </div>
        <button type="button" className={styles.addButton} onClick={startNew}>
          <PlusIcon />
          {mode === 'bots'
            ? t('settings.channels.addBot')
            : t('settings.channels.addTarget')}
        </button>
      </aside>

      <main className={styles.editor}>
        <div className={styles.editorHeader}>
          <div>
            <p>
              {mode === 'bots'
                ? t('settings.channels.botAccount')
                : t('settings.channels.target')}
            </p>
            <h3>
              {mode === 'bots'
                ? configured
                  ? botForm.id
                  : t('settings.channels.newBot')
                : configured
                  ? targetForm.id
                  : t('settings.channels.newTarget')}
            </h3>
          </div>
          <div className={styles.headerActions}>
            {selectedRevision !== undefined && (
              <span className={styles.revision}>rev {selectedRevision}</span>
            )}
            {mode === 'targets' && selectedTarget && (
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
                saving ||
                loading ||
                !authKey.trim() ||
                (configured && !dirty) ||
                (mode === 'targets' && botAccounts.length === 0)
              }
            >
              {saving ? t('settings.saving') : t('settings.save')}
            </button>
          </div>
        </div>

        <div className={styles.body}>
          {mode === 'bots' ? (
            <>
              {!configured && (
                <label className={styles.field}>
                  <span>{t('settings.channels.botId')}</span>
                  <input
                    aria-label={t('settings.channels.botId')}
                    value={botForm.id}
                    placeholder={t('settings.channels.botIdPlaceholder')}
                    onChange={(event) => updateBot('id', event.target.value)}
                  />
                </label>
              )}
              <section className={styles.section}>
                <div className={styles.sectionHeader}>
                  <p className={styles.sectionTitle}>
                    {t('settings.channels.telegramBot')}
                  </p>
                  <p className={styles.sectionCopy}>
                    {t('settings.channels.telegramBotCopy')}
                  </p>
                </div>
                <label className={styles.field}>
                  <span>{t('settings.channels.botToken')}</span>
                  <input
                    type="password"
                    autoComplete="off"
                    value={botForm.botToken}
                    placeholder={t('settings.channels.secretPlaceholder')}
                    onChange={(event) =>
                      updateBot('botToken', event.target.value)
                    }
                  />
                  <small>{t('settings.channels.botTokenHint')}</small>
                </label>
              </section>
              {configured && dirty && selectedBot?.bot_token_configured && (
                <div className={styles.warning}>
                  {t('settings.channels.secretUpdateWarning')}
                </div>
              )}
            </>
          ) : (
            <>
              {!configured && (
                <label className={styles.field}>
                  <span>{t('settings.channels.targetId')}</span>
                  <input
                    aria-label={t('settings.channels.targetId')}
                    value={targetForm.id}
                    placeholder={t('settings.channels.targetIdPlaceholder')}
                    onChange={(event) => updateTarget('id', event.target.value)}
                  />
                </label>
              )}
              <section className={styles.section}>
                <div className={styles.sectionHeader}>
                  <p className={styles.sectionTitle}>
                    {t('settings.channels.telegramTarget')}
                  </p>
                  <p className={styles.sectionCopy}>
                    {t('settings.channels.telegramTargetCopy')}
                  </p>
                </div>
                {botAccounts.length === 0 ? (
                  <div className={styles.warning}>
                    {t('settings.channels.botRequiredHint')}
                  </div>
                ) : (
                  <label className={styles.field}>
                    <span>{t('settings.channels.botAccount')}</span>
                    <select
                      aria-label={t('settings.channels.botAccount')}
                      value={targetForm.botAccountId}
                      onChange={(event) =>
                        updateTarget('botAccountId', event.target.value)
                      }
                    >
                      {botAccounts.map((account) => (
                        <option
                          value={account.bot_account_id}
                          key={account.bot_account_id}
                        >
                          {account.bot_account_id}
                        </option>
                      ))}
                    </select>
                  </label>
                )}
                <label className={styles.field}>
                  <span>{t('settings.channels.chatId')}</span>
                  <input
                    value={targetForm.chatId}
                    placeholder="-1001234567890"
                    onChange={(event) =>
                      updateTarget('chatId', event.target.value)
                    }
                  />
                  <small>{t('settings.channels.chatIdHint')}</small>
                </label>
              </section>
            </>
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

function upsertBotAccount(
  accounts: PublicBotAccount[],
  replacement: PublicBotAccount,
): PublicBotAccount[] {
  const next = accounts.filter(
    (account) => account.bot_account_id !== replacement.bot_account_id,
  );
  next.push(replacement);
  next.sort((left, right) =>
    left.bot_account_id.localeCompare(right.bot_account_id),
  );
  return next;
}

function upsertTarget(
  targets: PublicOutputChannel[],
  replacement: PublicOutputChannel,
): PublicOutputChannel[] {
  const next = targets.filter(
    (target) => target.output_channel_id !== replacement.output_channel_id,
  );
  next.push(replacement);
  next.sort((left, right) =>
    left.output_channel_id.localeCompare(right.output_channel_id),
  );
  return next;
}
