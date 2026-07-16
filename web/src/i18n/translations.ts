/**
 * Translation dictionaries for the supported locales.
 *
 * Keys are grouped by area but flattened into a single namespace: callers use
 * `t('key')`. Both locales must expose the same keys; the `Locale` type keeps
 * them in sync so a missing translation is a compile error.
 */

export const LOCALES = ['en', 'zh'] as const;
export type Locale = (typeof LOCALES)[number];

export const DEFAULT_LOCALE: Locale = 'en';
export const LOCALE_LABELS: Record<Locale, string> = {
  en: 'English',
  zh: '中文',
};

const en = {
  // Brand / generic
  'app.name': 'Phi',
  'app.empty.title': 'Phi',
  'app.empty.hint':
    'Choose an existing session or start a new chat from the sidebar.',
  'app.connecting': 'Connecting…',
  'app.preparing': 'Preparing session…',

  // Sidebar
  'sidebar.newChat': 'New chat',
  'sidebar.settings': 'Settings',
  'sidebar.noSessions': 'No sessions yet.',
  'sidebar.newSession': 'New session',
  'sidebar.preparing': 'preparing…',
  'sidebar.msg': 'msg',
  'sidebar.messages': 'messages',
  'sidebar.profile': 'profile',

  // Chat header / status
  'chat.status.awaiting_first_prompt': 'ready',
  'chat.status.idle': 'idle',
  'chat.status.compacting': 'compacting',
  'chat.status.running': 'running',
  'chat.status.stopping': 'stopping',
  'chat.status.closing': 'closing',
  'chat.status.closed': 'closed',
  'chat.status.offline': 'offline',
  'chat.connection.idle': 'no session',
  'chat.connection.connecting': 'connecting',
  'chat.connection.preparing': 'preparing',
  'chat.connection.error': 'connection failed',
  'chat.connection.retry': 'Retry',
  'chat.queued': 'queued',
  'chat.stop': 'Stop',
  'chat.contextTitle': '%pct% context used',
  'chat.composer.placeholder':
    'Send a message…  (Enter to send, Shift+Enter for newline)',
  'chat.composer.placeholderIdle': 'Start or select a session first',
  'chat.composer.placeholderConnecting': 'Connecting to session…',
  'chat.composer.placeholderPreparing': 'Preparing session…',
  'chat.composer.placeholderError': 'Session connection unavailable',
  'chat.composer.send': 'Send',
  'chat.composer.sendTitle': 'Send (Enter)',
  'chat.composer.stopTitle': 'Stop the active run',

  // Assistant / work detail
  'chat.assistant': 'assistant',
  'chat.thinking': 'Thinking…',
  'chat.workDetail': 'Work detail',
  'chat.workDetail.toggle': 'Toggle',
  'chat.workDetail.working': 'Working',
  'chat.workDetail.tool': 'tool',
  'chat.workDetail.tools': 'tools',
  'chat.workDetail.running': 'Running',
  'chat.workDetail.thinking': 'Thinking…',
  'chat.workDetail.stopped': 'Stopped',
  'chat.workDetail.beforeStop': 'before stop',
  'chat.workDetail.failed': 'Failed',
  'chat.workDetail.after': 'after',
  'chat.workDetail.completed': 'Completed',
  'chat.workDetail.ran': 'Ran',
  'chat.workDetail.turn': 'turn',
  'chat.workDetail.failedFlag': 'failed',
  'chat.tool': 'tool',
  'chat.toolResult': 'tool result',
  'chat.toolResultError': 'tool result (error)',
  'chat.toolResultEmpty': '<empty>',

  // Ask card
  'ask.badge': 'question',
  'ask.title': 'The assistant needs your input',
  'ask.singleSelect': 'single-select',
  'ask.multiSelect': 'multi-select',
  'ask.other': 'Other…',
  'ask.otherPlaceholder': 'Type a custom answer…',
  'ask.previewPlaceholder': 'Select an option to preview',
  'ask.submit': 'Submit answer',

  // Plan approval
  'plan.badge': 'plan · revision %{rev}',
  'plan.title': 'Approve this plan to exit plan mode',
  'plan.requestChanges': 'Request changes',
  'plan.approve': 'Approve plan',
  'plan.back': 'Back',
  'plan.sendFeedback': 'Send feedback',
  'plan.feedbackPlaceholder': 'Optional feedback for the assistant…',

  // Settings
  'settings.title': 'Settings',
  'settings.close': 'Close',
  'settings.closeLabel': 'Close settings',
  'settings.daemonConnection': 'Daemon connection',
  'settings.authKey': 'Auth key',
  'settings.authKeyPlaceholder':
    'The long-lived bearer key (PHI_DAEMON_AUTH_KEY_FILE)',
  'settings.authKeyHint':
    'Stored locally in your browser and sent as `Authorization: Bearer …` to the daemon.',
  'settings.providerProfile': 'Provider profile',
  'settings.profileId': 'Profile id',
  'settings.status': 'Status',
  'settings.configured': 'configured',
  'settings.notConfigured': 'not configured',
  'settings.providerAdapter': 'Provider adapter',
  'settings.model': 'Model',
  'settings.apiKey': 'API key',
  'settings.apiKeyPlaceholderConfigured':
    '•••••• (leave blank to keep existing)',
  'settings.apiKeyPlaceholder': 'provider api key',
  'settings.baseUrl': 'Base URL',
  'settings.maxContextTokens': 'max_context_tokens *',
  'settings.maxOutputTokens': 'max_output_tokens',
  'settings.temperature': 'temperature',
  'settings.reasoningEffort': 'reasoning_effort',
  'settings.maxRetries': 'max_retries',
  'settings.requestTimeoutSecs': 'request_timeout_secs',
  'settings.streamIdleTimeoutSecs': 'stream_idle_timeout_secs',
  'settings.saved': 'Provider profile saved.',
  'settings.save': 'Save',
  'settings.saving': 'Saving…',
  'settings.loading': 'Loading…',
  'settings.footerHint': 'Changes apply to new and re-attached sessions.',
  'settings.errors.apiKeyRequired': 'API key is required.',
  'settings.errors.baseUrlRequired': 'Base URL is required.',
  'settings.errors.modelRequired': 'Model is required.',
  'settings.errors.maxContext':
    'max_context_tokens must be a positive integer.',
  'settings.errors.authKeyRequired':
    'Daemon auth key is required to save the provider profile.',
  'settings.effortNone': '(none)',

  // Theme / language
  'theme.toggle': 'Toggle theme',
  'theme.dark': 'Dark',
  'theme.light': 'Light',
  'lang.toggle': 'Language',
} as const;

export type TranslationKey = keyof typeof en;
export type TranslationParams = Record<string, string | number>;

/** The full dictionary for a locale. `zh` is checked to match `en`'s keys. */
const zh: Record<TranslationKey, string> = {
  'app.name': 'Phi',
  'app.empty.title': 'Phi',
  'app.empty.hint': '请从侧边栏选择已有会话或新建对话。',
  'app.connecting': '连接中…',
  'app.preparing': '正在准备会话…',

  'sidebar.newChat': '新建对话',
  'sidebar.settings': '设置',
  'sidebar.noSessions': '暂无会话。',
  'sidebar.newSession': '新会话',
  'sidebar.preparing': '准备中…',
  'sidebar.msg': '条',
  'sidebar.messages': '条消息',
  'sidebar.profile': 'profile',

  'chat.status.awaiting_first_prompt': '就绪',
  'chat.status.idle': '空闲',
  'chat.status.compacting': '压缩中',
  'chat.status.running': '运行中',
  'chat.status.stopping': '停止中',
  'chat.status.closing': '关闭中',
  'chat.status.closed': '已关闭',
  'chat.status.offline': '离线',
  'chat.connection.idle': '未选择会话',
  'chat.connection.connecting': '连接中',
  'chat.connection.preparing': '准备中',
  'chat.connection.error': '连接失败',
  'chat.connection.retry': '重试',
  'chat.queued': '排队中',
  'chat.stop': '停止',
  'chat.contextTitle': '已用 %pct% 上下文',
  'chat.composer.placeholder': '发送消息…  (Enter 发送,Shift+Enter 换行)',
  'chat.composer.placeholderIdle': '请先新建或选择会话',
  'chat.composer.placeholderConnecting': '正在连接会话…',
  'chat.composer.placeholderPreparing': '正在准备会话…',
  'chat.composer.placeholderError': '会话连接不可用',
  'chat.composer.send': '发送',
  'chat.composer.sendTitle': '发送 (Enter)',
  'chat.composer.stopTitle': '停止当前运行',

  'chat.assistant': '助手',
  'chat.thinking': '思考中…',
  'chat.workDetail': '工作详情',
  'chat.workDetail.toggle': '展开/收起',
  'chat.workDetail.working': '工作中',
  'chat.workDetail.tool': '个工具',
  'chat.workDetail.tools': '个工具',
  'chat.workDetail.running': '正在运行',
  'chat.workDetail.thinking': '思考中…',
  'chat.workDetail.stopped': '已停止',
  'chat.workDetail.beforeStop': '于停止前',
  'chat.workDetail.failed': '失败',
  'chat.workDetail.after': '之后',
  'chat.workDetail.completed': '已完成',
  'chat.workDetail.ran': '运行了',
  'chat.workDetail.turn': '轮',
  'chat.workDetail.failedFlag': '失败',
  'chat.tool': '工具',
  'chat.toolResult': '工具结果',
  'chat.toolResultError': '工具结果(错误)',
  'chat.toolResultEmpty': '<空>',

  'ask.badge': '提问',
  'ask.title': '助手需要你的输入',
  'ask.singleSelect': '单选',
  'ask.multiSelect': '多选',
  'ask.other': '其他…',
  'ask.otherPlaceholder': '输入自定义回答…',
  'ask.previewPlaceholder': '选择一个选项以预览',
  'ask.submit': '提交回答',

  'plan.badge': '计划 · 第 %{rev} 版',
  'plan.title': '批准该计划以退出计划模式',
  'plan.requestChanges': '请求修改',
  'plan.approve': '批准计划',
  'plan.back': '返回',
  'plan.sendFeedback': '发送反馈',
  'plan.feedbackPlaceholder': '给助手的可选反馈…',

  'settings.title': '设置',
  'settings.close': '关闭',
  'settings.closeLabel': '关闭设置',
  'settings.daemonConnection': 'Daemon 连接',
  'settings.authKey': '鉴权密钥',
  'settings.authKeyPlaceholder': '长期 bearer key(PHI_DAEMON_AUTH_KEY_FILE)',
  'settings.authKeyHint':
    '保存在浏览器本地,并以 `Authorization: Bearer …` 发送给 daemon。',
  'settings.providerProfile': 'Provider profile',
  'settings.profileId': 'Profile id',
  'settings.status': '状态',
  'settings.configured': '已配置',
  'settings.notConfigured': '未配置',
  'settings.providerAdapter': 'Provider adapter',
  'settings.model': '模型',
  'settings.apiKey': 'API key',
  'settings.apiKeyPlaceholderConfigured': '•••••• (留空则保持不变)',
  'settings.apiKeyPlaceholder': 'provider api key',
  'settings.baseUrl': 'Base URL',
  'settings.maxContextTokens': 'max_context_tokens *',
  'settings.maxOutputTokens': 'max_output_tokens',
  'settings.temperature': 'temperature',
  'settings.reasoningEffort': 'reasoning_effort',
  'settings.maxRetries': 'max_retries',
  'settings.requestTimeoutSecs': 'request_timeout_secs',
  'settings.streamIdleTimeoutSecs': 'stream_idle_timeout_secs',
  'settings.saved': 'Provider profile 已保存。',
  'settings.save': '保存',
  'settings.saving': '保存中…',
  'settings.loading': '加载中…',
  'settings.footerHint': '更改将应用于新创建和重新连接的会话。',
  'settings.errors.apiKeyRequired': 'API key 不能为空。',
  'settings.errors.baseUrlRequired': 'Base URL 不能为空。',
  'settings.errors.modelRequired': '模型不能为空。',
  'settings.errors.maxContext': 'max_context_tokens 必须是正整数。',
  'settings.errors.authKeyRequired':
    '保存 provider profile 需要提供 daemon 鉴权密钥。',
  'settings.effortNone': '(无)',

  'theme.toggle': '切换主题',
  'theme.dark': '深色',
  'theme.light': '浅色',
  'lang.toggle': '语言',
};

export const translations: Record<Locale, Record<TranslationKey, string>> = {
  en,
  zh,
};

/**
 * Translate `key` for `locale`, substituting `%{name}` placeholders from
 * `params`. Falls back to English, then to the key itself.
 */
export function translate(
  locale: Locale,
  key: TranslationKey,
  params?: TranslationParams,
): string {
  const dict = translations[locale] ?? translations.en;
  let value = dict[key] ?? translations.en[key] ?? key;
  if (params) {
    for (const [name, replacement] of Object.entries(params)) {
      value = value.replace(`%{${name}}`, String(replacement));
    }
  }
  return value;
}
