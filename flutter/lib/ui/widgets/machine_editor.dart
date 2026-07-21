import 'package:flutter/material.dart';

import '../../app.dart';
import '../../core/models/connection_payload.dart';
import '../../core/models/machine_connection.dart';
import '../../core/transport/direct_transport.dart';
import '../../i18n/strings.dart';
import '../../platform/qr_scan_support.dart';
import '../../state/app_state.dart';
import '../../state/daemon_client.dart';
import '../pages/scan_connection_page.dart';

/// Shows the add/edit machine form as a full-screen route.
///
/// [existing] edits that machine; [prefill] (e.g. a scanned QR payload)
/// starts a new machine with the fields filled in. Saves directly through
/// [AppSettings]; new machines become active when [makeActive] is set.
Future<void> showMachineEditor(
  BuildContext context, {
  MachineConnection? existing,
  ConnectionPayload? prefill,
  bool makeActive = false,
}) {
  return Navigator.of(context).push(
    MaterialPageRoute<void>(
      builder: (_) => MachineEditorPage(
        existing: existing,
        prefill: prefill,
        makeActive: makeActive,
      ),
    ),
  );
}

/// Add/edit form for a single [MachineConnection]. The "test connection"
/// button probes the daemon with a throwaway transport built from the form
/// values — it never touches the persisted settings.
class MachineEditorPage extends StatefulWidget {
  const MachineEditorPage({
    super.key,
    this.existing,
    this.prefill,
    this.makeActive = false,
  });

  final MachineConnection? existing;
  final ConnectionPayload? prefill;
  final bool makeActive;

  @override
  State<MachineEditorPage> createState() => _MachineEditorPageState();
}

class _MachineEditorPageState extends State<MachineEditorPage> {
  late final TextEditingController _name;
  late final TextEditingController _baseUrl;
  late final TextEditingController _authKey;
  late bool _allowUntrustedCerts;
  bool _obscureKey = true;
  bool _testing = false;
  String? _testResult;
  bool _testOk = false;

  AppState get _app => AppScope.of(context);

  bool get _isEditing => widget.existing != null;

  @override
  void initState() {
    super.initState();
    final existing = widget.existing;
    final prefill = widget.prefill;
    _name = TextEditingController(text: existing?.name ?? prefill?.name ?? '');
    _baseUrl = TextEditingController(
      text: existing?.baseUrl ?? prefill?.baseUrl ?? '',
    );
    _authKey = TextEditingController(
      text: existing?.authKey ?? prefill?.authKey ?? '',
    );
    _allowUntrustedCerts = existing?.allowUntrustedCerts ?? false;
  }

  @override
  void dispose() {
    _name.dispose();
    _baseUrl.dispose();
    _authKey.dispose();
    super.dispose();
  }

  Future<void> _scanQr() async {
    final payload = await Navigator.of(context).push<ConnectionPayload>(
      MaterialPageRoute(builder: (_) => const ScanConnectionPage()),
    );
    if (payload == null || !mounted) return;
    setState(() {
      if (_name.text.trim().isEmpty) _name.text = payload.name;
      _baseUrl.text = payload.baseUrl;
      _authKey.text = payload.authKey;
      _testResult = null;
    });
  }

  Uri _normalizedUri() {
    var url = _baseUrl.text.trim();
    if (!url.startsWith('http://') && !url.startsWith('https://')) {
      url = 'http://$url';
    }
    return Uri.parse(url);
  }

  Future<void> _test() async {
    setState(() {
      _testing = true;
      _testResult = null;
    });
    final transport = DirectDaemonTransport(
      baseUri: _normalizedUri(),
      authKey: _authKey.text.trim(),
      allowUntrustedCerts: _allowUntrustedCerts,
    );
    try {
      final result = await DaemonClient(transport).listSessions();
      setState(() {
        _testOk = true;
        _testResult = S.of(context).connectedSessions(result.sessions.length);
      });
    } catch (error) {
      setState(() {
        _testOk = false;
        _testResult = '$error';
      });
    } finally {
      transport.dispose();
      setState(() => _testing = false);
    }
  }

  Future<void> _save() async {
    final s = S.of(context);
    if (_baseUrl.text.trim().isEmpty || _authKey.text.trim().isEmpty) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(s.machineUrlKeyRequired)));
      return;
    }
    final settings = _app.settings;
    final existing = widget.existing;
    if (existing != null) {
      await settings.updateMachine(
        existing.copyWith(
          name: _name.text,
          baseUrl: _baseUrl.text,
          authKey: _authKey.text,
          allowUntrustedCerts: _allowUntrustedCerts,
        ),
      );
    } else {
      await settings.addMachine(
        name: _name.text,
        baseUrl: _baseUrl.text,
        authKey: _authKey.text,
        allowUntrustedCerts: _allowUntrustedCerts,
        makeActive: widget.makeActive,
      );
    }
    if (mounted) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(s.machineSaved)));
      Navigator.of(context).pop();
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final s = S.of(context);
    return Scaffold(
      appBar: AppBar(
        title: Text(_isEditing ? s.editMachine : s.addMachine),
        actions: [TextButton(onPressed: _save, child: Text(s.save))],
      ),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          Align(
            alignment: Alignment.topCenter,
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 560),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  TextField(
                    controller: _name,
                    textCapitalization: TextCapitalization.words,
                    decoration: InputDecoration(
                      labelText: s.machineName,
                      hintText: s.machineNameHint,
                      border: const OutlineInputBorder(),
                      prefixIcon: const Icon(Icons.dns_outlined),
                    ),
                  ),
                  const SizedBox(height: 12),
                  TextField(
                    controller: _baseUrl,
                    keyboardType: TextInputType.url,
                    decoration: InputDecoration(
                      labelText: s.daemonUrl,
                      hintText: 'http://127.0.0.1:8787',
                      border: const OutlineInputBorder(),
                      prefixIcon: const Icon(Icons.link_rounded),
                      suffixIcon: qrScanSupported
                          ? IconButton(
                              tooltip: s.scanQrCode,
                              icon: const Icon(Icons.qr_code_scanner),
                              onPressed: _scanQr,
                            )
                          : null,
                    ),
                  ),
                  const SizedBox(height: 12),
                  TextField(
                    controller: _authKey,
                    obscureText: _obscureKey,
                    decoration: InputDecoration(
                      labelText: s.authKey,
                      hintText: s.authKeyHint,
                      border: const OutlineInputBorder(),
                      prefixIcon: const Icon(Icons.key_rounded),
                      suffixIcon: IconButton(
                        icon: Icon(
                          _obscureKey
                              ? Icons.visibility_outlined
                              : Icons.visibility_off_outlined,
                        ),
                        onPressed: () =>
                            setState(() => _obscureKey = !_obscureKey),
                      ),
                    ),
                  ),
                  const SizedBox(height: 8),
                  SwitchListTile(
                    contentPadding: EdgeInsets.zero,
                    title: Text(s.allowSelfSigned),
                    subtitle: Text(s.allowSelfSignedHint),
                    value: _allowUntrustedCerts,
                    onChanged: (value) =>
                        setState(() => _allowUntrustedCerts = value),
                  ),
                  const SizedBox(height: 8),
                  Wrap(
                    spacing: 12,
                    runSpacing: 8,
                    children: [
                      FilledButton.icon(
                        onPressed: _save,
                        icon: const Icon(Icons.save_outlined, size: 18),
                        label: Text(s.save),
                      ),
                      OutlinedButton.icon(
                        onPressed: _testing ? null : _test,
                        icon: _testing
                            ? const SizedBox(
                                width: 14,
                                height: 14,
                                child: CircularProgressIndicator(
                                  strokeWidth: 2,
                                ),
                              )
                            : const Icon(
                                Icons.wifi_tethering_rounded,
                                size: 18,
                              ),
                        label: Text(s.testConnection),
                      ),
                    ],
                  ),
                  if (_testResult != null) ...[
                    const SizedBox(height: 10),
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Icon(
                          _testOk
                              ? Icons.check_circle_outline
                              : Icons.error_outline,
                          size: 16,
                          color: _testOk
                              ? Colors.green
                              : theme.colorScheme.error,
                        ),
                        const SizedBox(width: 6),
                        Expanded(
                          child: Text(
                            _testResult!,
                            style: theme.textTheme.bodySmall?.copyWith(
                              color: _testOk
                                  ? Colors.green
                                  : theme.colorScheme.error,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ],
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}
