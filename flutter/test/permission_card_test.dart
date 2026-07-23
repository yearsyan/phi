import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/core/models/wire.dart';
import 'package:phi_client/ui/widgets/permission_card.dart';

const permissionRequest = ToolPermissionPrompt(
  permissionId: 'permission-1',
  call: ToolCall(id: 'call-1', name: 'bash', arguments: {'command': 'ls -la'}),
  effect: 'external_side_effect',
  capabilityMode: 'workspace_edit',
  suggestions: [
    ToolPermissionRule(toolName: 'bash', pattern: 'ls *'),
    ToolPermissionRule(toolName: 'bash', pattern: 'ls -la'),
  ],
);

void main() {
  test('permission wire models preserve pending prompts and decisions', () {
    final session = SessionDto.fromJson({
      'session_id': 'session-1',
      'config': {'model': 'test-model', 'revision': 0},
      'pending_tool_permissions': [
        {
          'permission_id': 'permission-1',
          'call': {
            'id': 'call-1',
            'name': 'bash',
            'arguments': {'command': 'ls -la'},
          },
          'effect': 'external_side_effect',
          'capability_mode': 'workspace_edit',
          'suggestions': [
            {'tool_name': 'bash', 'pattern': 'ls *'},
          ],
        },
      ],
    });
    expect(session.pendingToolPermissions.single.call.name, 'bash');
    expect(
      session.pendingToolPermissions.single.suggestions.single.label,
      'bash(ls *)',
    );

    final command =
        ClientCommand.decideToolPermission('request-1', 'permission-1', {
          'type': 'allow_for_session',
          'rule': const ToolPermissionRule(
            toolName: 'bash',
            pattern: 'ls *',
          ).toJson(),
        });
    expect(jsonDecode(command.encode()), {
      'type': 'decide_tool_permission',
      'request_id': 'request-1',
      'permission_id': 'permission-1',
      'decision': {
        'type': 'allow_for_session',
        'rule': {'tool_name': 'bash', 'pattern': 'ls *'},
      },
    });
  });

  testWidgets('submits a displayed server suggestion for the session', (
    tester,
  ) async {
    Json? decision;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: PermissionCard(
            request: permissionRequest,
            onDecision: (value) {
              decision = value;
              return true;
            },
          ),
        ),
      ),
    );

    expect(find.text('ls -la'), findsOneWidget);
    expect(find.text('bash(ls *)'), findsOneWidget);
    await tester.tap(find.widgetWithText(FilledButton, 'Allow for session'));
    await tester.pump();

    expect(decision, {
      'type': 'allow_for_session',
      'rule': {'tool_name': 'bash', 'pattern': 'ls *'},
    });
    await tester.pump(const Duration(seconds: 2));
  });

  testWidgets('supports one-shot approval', (tester) async {
    Json? decision;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: PermissionCard(
            request: permissionRequest,
            onDecision: (value) {
              decision = value;
              return true;
            },
          ),
        ),
      ),
    );

    await tester.tap(find.widgetWithText(OutlinedButton, 'Allow once'));
    await tester.pump();
    expect(decision, {'type': 'allow_once'});
    expect(
      tester
          .widget<OutlinedButton>(
            find.widgetWithText(OutlinedButton, 'Allow once'),
          )
          .onPressed,
      isNull,
    );
    await tester.pump(const Duration(seconds: 2));
    expect(
      tester
          .widget<OutlinedButton>(
            find.widgetWithText(OutlinedButton, 'Allow once'),
          )
          .onPressed,
      isNotNull,
    );
  });
}
