import 'package:flutter/widgets.dart';

import '../core/models/wire.dart';

/// Localized capability-mode label (shared across pages).
String capabilityModeLabel(S s, String mode) => switch (mode) {
  CapabilityMode.readOnly => s.capabilityReadOnly,
  CapabilityMode.fullAccess => s.capabilityFullAccess,
  _ => s.capabilityWorkspaceEdit,
};

/// Localized reasoning-effort label shown by the composer and its menu.
String reasoningEffortLabel(S s, String? effort) => switch (effort) {
  null => s.reasoningDefault,
  'none' => s.reasoningNone,
  'minimal' => s.reasoningMinimal,
  'low' => s.reasoningLow,
  'medium' => s.reasoningMedium,
  'high' => s.reasoningHigh,
  'xhigh' => s.reasoningExtraHigh,
  'max' => s.reasoningMaximum,
  _ => effort,
};

/// Hand-rolled app strings (en/zh), resolved from the widget locale.
///
/// Usage: `S.of(context).someKey`. The active locale comes from
/// `Localizations.localeOf(context)`, which MaterialApp sets from the
/// user's language preference (system / en / zh).
class S {
  const S(this.languageCode);

  final String languageCode;

  static S of(BuildContext context) =>
      S(Localizations.localeOf(context).languageCode);

  bool get _zh => languageCode == 'zh';

  String _t(String en, String zh) => _zh ? zh : en;

  /* ------------------------------- common -------------------------------- */
  String get cancel => _t('Cancel', '取消');
  String get delete => _t('Delete', '删除');
  String get retry => _t('Retry', '重试');
  String get save => _t('Save', '保存');
  String get close => _t('Close', '关闭');
  String get copy => _t('Copy', '复制');
  String get settings => _t('Settings', '设置');
  String get appTitle => _t('Phi', 'Phi');

  /* ----------------------------- home shell ------------------------------ */
  String get selectSessionHint =>
      _t('Select a session or start a new one', '选择一个会话，或开始新会话');

  /* ---------------------------- sessions page ---------------------------- */
  String get scheduledTasks => _t('Scheduled tasks', '计划任务');
  String get newSession => _t('New session', '新会话');
  String get filterSessions => _t('Filter sessions', '筛选会话');
  String get daemonNotConfigured => _t('Daemon not configured', '尚未配置 daemon');
  String get daemonNotConfiguredHint => _t(
    'Set the daemon URL and auth key in Settings.',
    '请在设置中填写 daemon 地址和密钥。',
  );
  String get openSettings => _t('Open settings', '打开设置');
  String get cannotReachDaemon => _t('Cannot reach daemon', '无法连接 daemon');
  String get noSessionsYet => _t('No sessions yet', '还没有会话');
  String get noMatches => _t('No matches', '没有匹配结果');
  String get startSessionHint =>
      _t('Start a new session to begin.', '开始一个新会话吧。');
  String get tryDifferentFilter => _t('Try a different filter.', '换个筛选条件试试。');
  String get deleteSessionTitle => _t('Delete session?', '删除会话？');
  String deleteSessionBody(String title) => _t(
    '“$title” and its transcript will be deleted permanently.',
    '“$title”及其聊天记录将被永久删除。',
  );
  String get noWorkspace => _t('No workspace', '无工作空间');
  String get untitledSession => _t('Untitled session', '未命名会话');
  String messageCount(int count) => _t('$count msgs', '$count 条消息');
  String queuedCount(int count) => _t('$count queued', '$count 条排队');
  String get pin => _t('Pin', '置顶');
  String get unpin => _t('Unpin', '取消置顶');
  String get copySessionId => _t('Copy session ID', '复制会话 ID');

  /* ------------------------------ chat page ------------------------------ */
  String get sessionIdCopied => _t('Session ID copied', '已复制会话 ID');
  String get deleteSessionConfirmBody => _t(
    'The session and its transcript will be deleted permanently.',
    '该会话及其聊天记录将被永久删除。',
  );
  String deleteFailed(Object error) =>
      _t('Delete failed: $error', '删除失败：$error');
  String forkFailed(Object error) => _t('Fork failed: $error', '分叉失败：$error');
  String get forkPredatesCompaction => _t(
    'Cannot fork this reply: it predates the last compaction.',
    '无法从这条回复分叉：它位于最近一次压缩之前。',
  );
  String get forkedIntoNewSession => _t('Forked into a new session', '已分叉到新会话');
  String get compactContext => _t('Compact context', '压缩上下文');
  String get reconnect => _t('Reconnect', '重新连接');
  String get sessionActions => _t('Session actions', '会话操作');
  String get deleteSession => _t('Delete session', '删除会话');
  String get statusRunning => _t('running', '运行中');
  String get statusCompacting => _t('compacting', '压缩中');
  String get statusStopping => _t('stopping', '停止中');
  String get statusOffline => _t('offline', '离线');
  String get statusClosed => _t('closed', '已关闭');
  String get statusReady => _t('ready', '就绪');
  String get statusIdle => _t('idle', '空闲');
  String queuedSuffix(int count) => _t('$count queued', '$count 排队');
  String reconnectingWithError(String error) =>
      _t('Reconnecting — $error', '正在重连 — $error');
  String get reconnecting => _t('Reconnecting…', '正在重连…');
  String get connectionFailed => _t('Connection failed', '连接失败');
  String get connectionNotOpen =>
      _t('The session connection is not open.', '会话连接未打开。');

  /* ------------------------------ composer ------------------------------- */
  String get messageHint =>
      _t('Message phi…  (/ for commands)', '向 phi 提问…（/ 命令）');
  String get queueMessageHint => _t('Queue a message…', '排队发送消息…');
  String get connectingHint => _t('Connecting…', '连接中…');
  String get stop => _t('Stop', '停止');
  String get send => _t('Send', '发送');
  String get voiceInput => _t('Voice input', '语音输入');
  String get addImages => _t('Add images', '添加图片');
  String removeImage(String name) => _t('Remove $name', '移除 $name');
  String imagePickerFailed(Object error) =>
      _t('Could not add the image: $error', '无法添加图片：$error');
  String imageLimitReached(int count) =>
      _t('You can attach up to $count images.', '最多可添加 $count 张图片。');
  String get queueMessage => _t('Queue message', '排队发送');
  String capabilityModeTooltip(String label) =>
      _t('Capability mode: $label', '能力模式：$label');
  String get capabilityReadOnly => _t('Read only', '只读');
  String get capabilityWorkspaceEdit => _t('Workspace edit', '工作区编辑');
  String get capabilityFullAccess => _t('Full access', '完全访问');
  String reasoningEffortTooltip(String effort) =>
      _t('Reasoning effort: $effort', '推理强度：$effort');
  String get reasoningDefault => _t('Default', '默认');
  String get reasoningNone => _t('Off', '关闭');
  String get reasoningMinimal => _t('Minimal', '最低');
  String get reasoningLow => _t('Low', '低');
  String get reasoningMedium => _t('Medium', '中');
  String get reasoningHigh => _t('High', '高');
  String get reasoningExtraHigh => _t('Extra high', '极高');
  String get reasoningMaximum => _t('Maximum', '最高');
  String get modelAndReasoning => _t('Model and reasoning', '模型与推理');
  String get modelLabel => _t('Model', '模型');
  String get reasoningEffort => _t('Reasoning effort', '推理强度');
  String get setModel => _t('Set model', '设置模型');
  String get modelHint =>
      _t('e.g. claude-sonnet-4-5, gpt-5.2', '例如 claude-sonnet-4-5、gpt-5.2');
  String get setAction => _t('Set', '设置');
  String get compactDescription =>
      _t('Compact the conversation context', '压缩当前会话的上下文');
  String get customModel => _t('Custom model…', '自定义模型…');
  String get chooseModel => _t('Choose model', '选择模型');
  String get customModelHint => _t(
    'Enter a model name for the current provider profile.',
    '为当前 provider 配置输入自定义模型名。',
  );
  String contextTokens(String used, String max) =>
      _t('Context: $used / $max tokens', '上下文：$used / $max tokens');

  /* ------------------------------- timeline ------------------------------ */
  String get sendMessageToStart => _t('Send a message to start', '发一条消息开始吧');
  String get thinkingStreaming => _t('Thinking…', '思考中…');
  String get thinking => _t('Thinking', '思考过程');
  String get forkFromReply => _t('Fork from this reply', '从这条回复分叉');
  String get arguments => _t('Arguments', '参数');
  String get progress => _t('Progress', '进度');
  String get result => _t('Result', '结果');
  String get errorLabel => _t('Error', '错误');
  String usedTools(int count) =>
      _t('Used $count tool${count == 1 ? '' : 's'}', '使用了 $count 个工具');
  String failedCount(int count) => _t('$count failed', '$count 个失败');
  String retriesCount(int count) =>
      _t('$count ${count == 1 ? 'retry' : 'retries'}', '$count 次重试');
  String stepsCount(int count) =>
      _t('$count step${count == 1 ? '' : 's'}', '$count 个步骤');
  String get runCompleted => _t('Run completed', '运行完成');
  String get runStopped => _t('Run stopped', '运行已停止');
  String runFailed(String? message) =>
      _t('Run failed: ${message ?? ''}', '运行失败：${message ?? ''}');
  String get compactingContext => _t('Compacting context…', '正在压缩上下文…');
  String get contextCompacted => _t('Context compacted', '上下文已压缩');
  String compactionFailed(String? message) => _t(
    'Compaction failed${message != null ? ': $message' : ''}',
    '压缩失败${message != null ? '：$message' : ''}',
  );
  String get working => _t('Working…', '正在工作…');
  String get emptyMessage => _t('(empty message)', '（空消息）');
  String get sending => _t('sending…', '发送中…');
  String queuedAt(int? position) => _t(
    position != null ? 'queued #$position' : 'queued',
    '排队中${position != null ? ' #$position' : ''}',
  );
  String providerRetry(int number, int max, String reason) => _t(
    'Provider retry $number/$max: $reason',
    'Provider 重试 $number/$max：$reason',
  );
  String get truncated => _t('… (truncated)', '…（已截断）');
  String get copyCode => _t('Copy code', '复制代码');

  /* ------------------------------- ask card ------------------------------ */
  String get phiAsks => _t('phi asks', 'phi 提问');
  String get otherHint => _t('Other…', '其他…');
  String get answer => _t('Answer', '回答');
  String get sent => _t('Sent', '已发送');

  /* --------------------------- workspace picker -------------------------- */
  String get noWorkspaceDefault =>
      _t('No workspace (daemon default)', '无工作空间（daemon 默认）');
  String get browse => _t('Browse', '浏览');
  String get capabilityMode => _t('Capability mode', '能力模式');
  String get start => _t('Start', '开始');
  String get chooseWorkspace => _t('Choose workspace', '选择工作空间');
  String get parentDirectory => _t('Parent directory', '上级目录');
  String get noSubdirectories => _t('No subdirectories', '没有子目录');
  String get selectThisDirectory => _t('Select this directory', '选择此目录');
  String get providerProfile => _t('Provider profile', 'Provider 配置');

  /* --------------------------- scheduled tasks --------------------------- */
  String get newTask => _t('New task', '新建任务');
  String get noScheduledTasks => _t('No scheduled tasks yet', '还没有计划任务');
  String get runNow => _t('Run now', '立即运行');
  String get openLastRun => _t('Open last run', '打开最近运行');
  String deleteTaskTitle(String name) => _t('Delete “$name”?', '删除“$name”？');
  String get nameLabel => _t('Name', '名称');
  String get promptLabel => _t('Prompt', '提示词');
  String get workspaceLabel => _t('Workspace', '工作空间');
  String get intervalLabel => _t('Interval', '间隔');
  String get dailyLabel => _t('Daily', '每天');
  String get everyLabel => _t('Every', '每');
  String get minutesLabel => _t('minutes', '分钟');
  String get hoursLabel => _t('hours', '小时');
  String get daysLabel => _t('days', '天');
  String get timezoneLabel => _t('Timezone (IANA)', '时区（IANA）');
  String get create => _t('Create', '创建');
  String get namePromptRequired =>
      _t('Name and prompt are required.', '名称和提示词必填。');
  String get runStarted => _t('Run started', '已启动运行');
  String nextRun(String time) => _t('next $time', '下次 $time');

  /* ------------------------------ settings ------------------------------- */
  String get daemonConnection => _t('Daemon connection', 'Daemon 连接');
  String get daemonConnectionDescription => _t(
    'The app talks to a phi daemon over HTTP(S) for REST and '
        'WebSocket for session streaming. Transports are '
        'pluggable; direct connection is used here.',
    '应用通过 HTTP(S) 访问 daemon 的 REST 接口，通过 WebSocket 接收会话流。'
        '通信层是可插拔的，当前使用直连。',
  );
  String get daemonUrl => _t('Daemon URL', 'Daemon 地址');
  String get authKey => _t('Auth key', '密钥');
  String get authKeyHint => _t(
    'Contents of PHI_DAEMON_AUTH_KEY_FILE',
    'PHI_DAEMON_AUTH_KEY_FILE 文件的内容',
  );
  String get allowSelfSigned => _t('Allow self-signed certificates', '允许自签名证书');
  String get allowSelfSignedHint => _t(
    'Only enable on networks you trust (HTTPS daemons with untrusted certs)',
    '仅在可信网络中开启（用于证书不受信的 HTTPS daemon）',
  );
  String get testConnection => _t('Test connection', '测试连接');
  String get scanQrCode => _t('Scan QR code', '扫描二维码');
  String get scanToConnect => _t('Scan to connect', '扫码连接');
  String get scanQrHint => _t(
    'Point the camera at the connection QR code printed by phi-daemon '
        'when it starts.',
    '对准 phi-daemon 启动时在终端打印的连接二维码。',
  );
  String get invalidQrCode =>
      _t('Not a phi-daemon connection QR code', '不是有效的 phi-daemon 连接二维码');
  String get cameraPermissionDenied => _t(
    'Camera access was denied. Enable it in system settings to scan the '
        'connection QR code.',
    '相机权限被拒绝。请在系统设置中允许访问相机后再扫码。',
  );
  String get cameraUnavailable =>
      _t('Camera is unavailable on this device.', '此设备的相机不可用。');
  String get settingsSaved => _t('Settings saved', '设置已保存');
  String connectedSessions(int count) => _t(
    'Connected — $count session(s) on daemon.',
    '连接成功，daemon 上有 $count 个会话。',
  );
  String get defaults => _t('Defaults', '默认值');
  String get defaultCapabilityMode =>
      _t('Default capability mode for new sessions', '新会话的默认能力模式');
  String get language => _t('Language', '语言');
  String get languageSystem => _t('System', '跟随系统');

  /* ------------------------------ usage detail --------------------------- */
  String get contextUsageTitle => _t('Context usage', '上下文用量');
  String get usedTokens => _t('Used', '已用');
  String get remainingTokens => _t('Remaining', '剩余');
  String get maxTokens => _t('Max', '上限');
  String get lastCallTokens => _t('Last call', '最近一次调用');
  String get cumulativeTokens => _t('Cumulative', '累计');
  String get inputTokens => _t('Input', '输入');
  String get outputTokens => _t('Output', '输出');
  String get cachedTokens => _t('Cached input', '缓存输入');
  String get noUsageYet => _t('No usage data yet.', '还没有用量数据。');
}
