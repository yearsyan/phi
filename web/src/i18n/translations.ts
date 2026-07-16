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
  'sidebar.newChatHint': 'Start a fresh coding task',
  'sidebar.settings': 'Settings',
  'sidebar.sessions': 'Sessions',
  'sidebar.close': 'Close session navigation',
  'sidebar.recent': 'Recent sessions',
  'sidebar.noSessions': 'No sessions yet.',
  'sidebar.noSessionsHint': 'Your activated sessions will appear here.',
  'sidebar.newSession': 'New session',
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
  'chat.connection.reconnecting': 'reconnecting',
  'chat.connection.error': 'connection failed',
  'chat.connection.retry': 'Retry',
  'chat.connection.waitHint':
    'Authenticating with the daemon and synchronizing the session.',
  'chat.queued': 'queued',
  'chat.stop': 'Stop',
  'chat.contextTitle': '%pct% context used',
  'chat.composer.placeholder':
    'Send a message…  (Enter to send, Shift+Enter for newline)',
  'chat.composer.placeholderIdle': 'Start or select a session first',
  'chat.composer.placeholderConnecting': 'Connecting to session…',
  'chat.composer.placeholderPreparing': 'Preparing session…',
  'chat.composer.placeholderError': 'Session connection unavailable',
  'chat.composer.placeholderUnavailable': 'Session connection unavailable',
  'chat.composer.placeholderQueue':
    'Add another instruction — it will join the session queue…',
  'chat.composer.ariaLabel': 'Message Phi',
  'chat.composer.enter': 'Enter to send',
  'chat.composer.shiftEnter': 'Shift+Enter for a new line',
  'chat.composer.queued': '%{count} queued',
  'chat.composer.queue': 'Queue',
  'chat.composer.disclaimer':
    'Phi can edit files and run commands in the configured workspace.',
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
  'chat.mode.default': 'Build',
  'chat.mode.plan': 'Plan',
  'chat.capability.label': 'Access',
  'chat.capability.title':
    'Choose the maximum tool capability for this session',
  'chat.capability.readOnly': 'Read only',
  'chat.capability.workspaceEdit': 'Workspace edit',
  'chat.capability.fullAccess': 'Full access',
  'chat.compact': 'Compact',
  'chat.prompt.sending': 'Sending…',
  'chat.prompt.queued': 'Queued at position %{position}',
  'chat.notice.dismiss': 'Dismiss notice',
  'chat.jumpToBottom': 'Latest',
  'chat.toolGroup': '%{count} tool calls',
  'chat.activity.runningTools': 'Working · %{count} tools',
  'chat.activity.thinking': 'Thinking through the task',
  'chat.activity.failed': 'Run failed',
  'chat.activity.stopped': 'Run stopped',
  'chat.activity.completed': 'Work completed · %{count} tools',
  'chat.activity.waiting': 'Waiting for the next model action…',
  'chat.activity.error': 'Error',
  'chat.activity.retry': 'Retry',
  'chat.activity.compaction': 'Context',
  'chat.activity.subagent': 'Subagent',
  'chat.activity.toolRunning': 'Running…',
  'chat.activity.toolFailed': 'Failed',
  'chat.activity.toolDone': 'Completed',
  'chat.activity.details': 'Show details',
  'chat.activity.arguments': 'Arguments',
  'chat.activity.output': 'Output',
  'chat.welcome.eyebrow': 'Interactive coding agent',
  'chat.welcome.title': 'What should we change?',
  'chat.welcome.copy':
    'Describe the outcome you want. Phi will inspect the workspace, make focused edits, and verify the result.',
  'chat.welcome.inspect': 'Inspect the project',
  'chat.welcome.inspectPrompt':
    'Inspect this repository and summarize its architecture, current state, and highest-priority issues.',
  'chat.welcome.fix': 'Fix a problem',
  'chat.welcome.fixPrompt':
    'Find the most important reproducible bug in this project, fix it, and verify the change.',
  'chat.welcome.explain': 'Review recent work',
  'chat.welcome.explainPrompt':
    'Review the current working tree, explain the changes, and call out any risks or missing tests.',

  // Ask card
  'ask.badge': 'question',
  'ask.title': 'The assistant needs your input',
  'ask.singleSelect': 'single-select',
  'ask.multiSelect': 'multi-select',
  'ask.other': 'Other…',
  'ask.otherPlaceholder': 'Type a custom answer…',
  'ask.previewPlaceholder': 'Select an option to preview',
  'ask.submit': 'Submit answer',
  'ask.submitting': 'Submitting…',

  // Plan approval
  'plan.badge': 'plan · revision %{rev}',
  'plan.title': 'Approve this plan to exit plan mode',
  'plan.requestChanges': 'Request changes',
  'plan.approve': 'Approve plan',
  'plan.back': 'Back',
  'plan.sendFeedback': 'Send feedback',
  'plan.submitting': 'Submitting…',
  'plan.feedbackPlaceholder': 'Optional feedback for the assistant…',

  // Settings
  'settings.title': 'Settings',
  'settings.eyebrow': 'Workspace connection',
  'settings.close': 'Close',
  'settings.closeLabel': 'Close settings',
  'settings.daemonConnection': 'Daemon connection',
  'settings.connectionCopy':
    'The daemon key authorizes session access, file edits, and command execution.',
  'settings.load': 'Connect & load',
  'settings.authKey': 'Auth key',
  'settings.authKeyPlaceholder':
    'The long-lived bearer key (PHI_DAEMON_AUTH_KEY_FILE)',
  'settings.authKeyHint':
    'Stored locally in your browser and sent as `Authorization: Bearer …` to the daemon.',
  'settings.providerProfile': 'Provider profile',
  'settings.providerCopy':
    'Choose an existing profile or type a new profile id to create one.',
  'settings.profileId': 'Profile id',
  'settings.sessionDefaults': 'New session defaults',
  'settings.sessionDefaultsCopy':
    'Optionally choose an Agent Profile and capability mode for newly prepared sessions.',
  'settings.agentProfileId': 'Agent Profile id (optional)',
  'settings.agentProfileIdPlaceholder': 'default',
  'settings.capabilityMode': 'Capability mode',
  'settings.capabilityProfileDefault': 'Use Agent Profile default',
  'settings.status': 'Status',
  'settings.configured': 'configured',
  'settings.notConfigured': 'not configured',
  'settings.providerAdapter': 'Provider adapter',
  'settings.model': 'Model',
  'settings.apiKey': 'API key',
  'settings.apiKeyPlaceholderConfigured':
    'Stored by daemon; re-enter before updating',
  'settings.apiKeyRequiredToUpdate':
    'Re-enter the provider API key to update this profile',
  'settings.apiKeyPlaceholder': 'provider api key',
  'settings.baseUrl': 'Base URL',
  'settings.maxContextTokens': 'max_context_tokens *',
  'settings.maxOutputTokens': 'max_output_tokens',
  'settings.temperature': 'temperature',
  'settings.reasoningEffort': 'reasoning_effort',
  'settings.maxRetries': 'max_retries',
  'settings.requestTimeoutSecs': 'request_timeout_secs',
  'settings.streamIdleTimeoutSecs': 'stream_idle_timeout_secs',
  'settings.advanced': 'Advanced provider settings',
  'settings.apiKeyUpdateWarning':
    'The daemon never returns stored provider credentials. Re-enter the provider API key before saving profile changes.',
  'settings.saved': 'Provider profile saved.',
  'settings.save': 'Save',
  'settings.saving': 'Saving…',
  'settings.loading': 'Loading…',
  'settings.footerHint': 'Changes apply to new and re-attached sessions.',
  'settings.errors.apiKeyRequired': 'API key is required.',
  'settings.errors.apiKeyRequiredOnWrite':
    'Enter the provider API key to create or update this profile.',
  'settings.errors.baseUrlRequired': 'Base URL is required.',
  'settings.errors.modelRequired': 'Model is required.',
  'settings.errors.maxContext':
    'max_context_tokens must be a positive integer.',
  'settings.errors.maxOutput':
    'max_output_tokens must be a positive integer when provided.',
  'settings.errors.advancedNumbers':
    'Retries must be non-negative and timeout values must be positive integers.',
  'settings.errors.temperature': 'temperature must be a valid number.',
  'settings.errors.profileNotConfigured':
    'This provider profile is not configured for the supplied daemon key.',
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
  'sidebar.newChatHint': '开始一个新的编码任务',
  'sidebar.settings': '设置',
  'sidebar.sessions': '会话',
  'sidebar.close': '关闭会话导航',
  'sidebar.recent': '最近会话',
  'sidebar.noSessions': '暂无会话。',
  'sidebar.noSessionsHint': '激活后的会话会显示在这里。',
  'sidebar.newSession': '新会话',
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
  'chat.connection.reconnecting': '正在重连',
  'chat.connection.error': '连接失败',
  'chat.connection.retry': '重试',
  'chat.connection.waitHint': '正在验证 daemon 并同步会话状态。',
  'chat.queued': '排队中',
  'chat.stop': '停止',
  'chat.contextTitle': '已用 %pct% 上下文',
  'chat.composer.placeholder': '发送消息…  (Enter 发送,Shift+Enter 换行)',
  'chat.composer.placeholderIdle': '请先新建或选择会话',
  'chat.composer.placeholderConnecting': '正在连接会话…',
  'chat.composer.placeholderPreparing': '正在准备会话…',
  'chat.composer.placeholderError': '会话连接不可用',
  'chat.composer.placeholderUnavailable': '会话连接不可用',
  'chat.composer.placeholderQueue': '继续输入指令，它会加入当前会话队列…',
  'chat.composer.ariaLabel': '给 Phi 发送消息',
  'chat.composer.enter': 'Enter 发送',
  'chat.composer.shiftEnter': 'Shift+Enter 换行',
  'chat.composer.queued': '%{count} 条排队中',
  'chat.composer.queue': '排队',
  'chat.composer.disclaimer': 'Phi 可以在配置的工作区内修改文件并执行命令。',
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
  'chat.mode.default': '执行',
  'chat.mode.plan': '计划',
  'chat.capability.label': '权限',
  'chat.capability.title': '选择当前会话允许使用的最高工具能力',
  'chat.capability.readOnly': '只读',
  'chat.capability.workspaceEdit': '允许工作区编辑',
  'chat.capability.fullAccess': '完全允许',
  'chat.compact': '压缩',
  'chat.prompt.sending': '正在发送…',
  'chat.prompt.queued': '已排到第 %{position} 位',
  'chat.notice.dismiss': '关闭通知',
  'chat.jumpToBottom': '最新消息',
  'chat.toolGroup': '%{count} 个工具调用',
  'chat.activity.runningTools': '工作中 · %{count} 个工具',
  'chat.activity.thinking': '正在分析任务',
  'chat.activity.failed': '运行失败',
  'chat.activity.stopped': '运行已停止',
  'chat.activity.completed': '工作完成 · %{count} 个工具',
  'chat.activity.waiting': '等待下一步模型操作…',
  'chat.activity.error': '错误',
  'chat.activity.retry': '重试',
  'chat.activity.compaction': '上下文',
  'chat.activity.subagent': '子 Agent',
  'chat.activity.toolRunning': '正在运行…',
  'chat.activity.toolFailed': '失败',
  'chat.activity.toolDone': '已完成',
  'chat.activity.details': '查看详情',
  'chat.activity.arguments': '参数',
  'chat.activity.output': '输出',
  'chat.welcome.eyebrow': '交互式编码 Agent',
  'chat.welcome.title': '这次要改什么？',
  'chat.welcome.copy':
    '直接描述你想要的结果。Phi 会检查工作区、完成聚焦修改，并验证最终结果。',
  'chat.welcome.inspect': '检查项目',
  'chat.welcome.inspectPrompt':
    '检查这个仓库，概括它的架构、当前状态以及最需要优先解决的问题。',
  'chat.welcome.fix': '修复问题',
  'chat.welcome.fixPrompt':
    '找出这个项目中最重要且可复现的 bug，修复它并验证修改。',
  'chat.welcome.explain': '审查当前改动',
  'chat.welcome.explainPrompt':
    '审查当前工作区改动，解释这些变化，并指出风险或缺失的测试。',

  'ask.badge': '提问',
  'ask.title': '助手需要你的输入',
  'ask.singleSelect': '单选',
  'ask.multiSelect': '多选',
  'ask.other': '其他…',
  'ask.otherPlaceholder': '输入自定义回答…',
  'ask.previewPlaceholder': '选择一个选项以预览',
  'ask.submit': '提交回答',
  'ask.submitting': '提交中…',

  'plan.badge': '计划 · 第 %{rev} 版',
  'plan.title': '批准该计划以退出计划模式',
  'plan.requestChanges': '请求修改',
  'plan.approve': '批准计划',
  'plan.back': '返回',
  'plan.sendFeedback': '发送反馈',
  'plan.submitting': '提交中…',
  'plan.feedbackPlaceholder': '给助手的可选反馈…',

  'settings.title': '设置',
  'settings.eyebrow': '工作区连接',
  'settings.close': '关闭',
  'settings.closeLabel': '关闭设置',
  'settings.daemonConnection': 'Daemon 连接',
  'settings.connectionCopy': 'Daemon key 会授权会话访问、文件修改和命令执行。',
  'settings.load': '连接并加载',
  'settings.authKey': '鉴权密钥',
  'settings.authKeyPlaceholder': '长期 bearer key(PHI_DAEMON_AUTH_KEY_FILE)',
  'settings.authKeyHint':
    '保存在浏览器本地,并以 `Authorization: Bearer …` 发送给 daemon。',
  'settings.providerProfile': 'Provider profile',
  'settings.providerCopy': '选择已有 profile，或输入新的 profile id 创建配置。',
  'settings.profileId': 'Profile id',
  'settings.sessionDefaults': '新会话默认值',
  'settings.sessionDefaultsCopy':
    '可为新准备的会话指定 Agent Profile 和能力模式。',
  'settings.agentProfileId': 'Agent Profile id（可选）',
  'settings.agentProfileIdPlaceholder': 'default',
  'settings.capabilityMode': '能力模式',
  'settings.capabilityProfileDefault': '使用 Agent Profile 默认值',
  'settings.status': '状态',
  'settings.configured': '已配置',
  'settings.notConfigured': '未配置',
  'settings.providerAdapter': 'Provider adapter',
  'settings.model': '模型',
  'settings.apiKey': 'API key',
  'settings.apiKeyPlaceholderConfigured': 'Daemon 已保存；更新时需重新输入',
  'settings.apiKeyRequiredToUpdate':
    '更新 profile 时需要重新输入 Provider API key',
  'settings.apiKeyPlaceholder': 'provider api key',
  'settings.baseUrl': 'Base URL',
  'settings.maxContextTokens': 'max_context_tokens *',
  'settings.maxOutputTokens': 'max_output_tokens',
  'settings.temperature': 'temperature',
  'settings.reasoningEffort': 'reasoning_effort',
  'settings.maxRetries': 'max_retries',
  'settings.requestTimeoutSecs': 'request_timeout_secs',
  'settings.streamIdleTimeoutSecs': 'stream_idle_timeout_secs',
  'settings.advanced': '高级 Provider 设置',
  'settings.apiKeyUpdateWarning':
    'Daemon 不会返回已保存的凭据。修改 profile 后保存时，请重新输入 Provider API key。',
  'settings.saved': 'Provider profile 已保存。',
  'settings.save': '保存',
  'settings.saving': '保存中…',
  'settings.loading': '加载中…',
  'settings.footerHint': '更改将应用于新创建和重新连接的会话。',
  'settings.errors.apiKeyRequired': 'API key 不能为空。',
  'settings.errors.apiKeyRequiredOnWrite':
    '创建或更新 profile 时必须输入 Provider API key。',
  'settings.errors.baseUrlRequired': 'Base URL 不能为空。',
  'settings.errors.modelRequired': '模型不能为空。',
  'settings.errors.maxContext': 'max_context_tokens 必须是正整数。',
  'settings.errors.maxOutput': '填写 max_output_tokens 时必须使用正整数。',
  'settings.errors.advancedNumbers':
    '重试次数必须为非负整数，超时必须为正整数。',
  'settings.errors.temperature': 'temperature 必须是有效数字。',
  'settings.errors.profileNotConfigured':
    '使用当前 daemon key 时，该 Provider profile 尚未配置。',
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
