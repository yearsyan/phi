import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/core/models/wire.dart';

void main() {
  test(
    'Agent Profile wire model keeps MCP references and policy semantics',
    () {
      final profile = PublicAgentProfile.fromJson({
        'agent_profile_id': 'reviewer',
        'revision': 3,
        'prompt': {'mode': 'full', 'text': 'Review only.'},
        'tools': {
          'allow': ['read', 'github__search'],
          'deny': ['bash'],
        },
        'skills': {'allow': null, 'deny': []},
        'mcp_profile_ids': ['github', 'local-tools'],
        'initial_capability_mode': 'full_access',
        'model': 'review-model',
        'reasoning_effort': 'high',
      });

      expect(profile.agentProfileId, 'reviewer');
      expect(profile.promptMode, 'full');
      expect(profile.toolAllow, ['read', 'github__search']);
      expect(profile.skillAllow, isNull);
      expect(profile.mcpProfileIds, ['github', 'local-tools']);
      expect(profile.initialCapabilityMode, 'full_access');
    },
  );

  test('MCP Profile wire model parses redacted HTTP and stdio transports', () {
    final remote = PublicMcpProfile.fromJson({
      'mcp_profile_id': 'remote',
      'revision': 2,
      'transport': {
        'type': 'http',
        'url': 'https://mcp.example.test/rpc',
        'bearer_token_configured': true,
        'header_names': ['x-api-key'],
        'allow_stateless': false,
        'reinitialize_on_expired_session': true,
      },
      'tool_name_prefix': 'remote',
      'connect_timeout_secs': 12,
      'request_timeout_secs': null,
      'max_output_lines': 123,
      'max_output_bytes': 4567,
    });
    expect(remote.transport.type, 'http');
    expect(remote.transport.bearerTokenConfigured, isTrue);
    expect(remote.transport.headerNames, ['x-api-key']);
    expect(remote.requestTimeoutSecs, isNull);

    final local = PublicMcpProfile.fromJson({
      'mcp_profile_id': 'local',
      'revision': 1,
      'transport': {
        'type': 'stdio',
        'command': 'npx',
        'args': ['-y', '@example/mcp'],
        'current_dir': 'tools',
        'env_keys': ['MCP_TOKEN'],
        'clear_env': true,
      },
      'tool_name_prefix': 'local',
      'connect_timeout_secs': 30,
      'request_timeout_secs': 60,
      'max_output_lines': 2000,
      'max_output_bytes': 51200,
    });
    expect(local.transport.type, 'stdio');
    expect(local.transport.command, 'npx');
    expect(local.transport.args, ['-y', '@example/mcp']);
    expect(local.transport.envKeys, ['MCP_TOKEN']);
    expect(local.transport.clearEnv, isTrue);
  });

  test('Output channel wire model exposes Telegram metadata but no token', () {
    final channel = PublicOutputChannel.fromJson({
      'type': 'telegram',
      'output_channel_id': 'alerts',
      'revision': 4,
      'bot_token_configured': true,
      'chat_id': '-1001234567890',
    });

    expect(channel.type, 'telegram');
    expect(channel.outputChannelId, 'alerts');
    expect(channel.revision, 4);
    expect(channel.botTokenConfigured, isTrue);
    expect(channel.chatId, '-1001234567890');
  });

  test(
    'Scheduled task output channel is optional for older daemon responses',
    () {
      ScheduledTask task(Map<String, dynamic> extra) => ScheduledTask.fromJson({
        'task_id': 'task-1',
        'name': 'Review',
        'prompt': 'Review the workspace',
        'schedule': {'type': 'interval', 'every': 1, 'unit': 'hours'},
        ...extra,
      });

      expect(task({}).outputChannelId, isNull);
      expect(task({'output_channel_id': 'alerts'}).outputChannelId, 'alerts');
    },
  );
}
