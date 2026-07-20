import 'package:flutter/material.dart';

import '../../app.dart';
import '../../core/models/wire.dart';
import '../../i18n/strings.dart';
import '../../state/app_state.dart';

/// Connection settings: daemon address, auth key, TLS policy, plus default
/// capability mode for new sessions.
class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key, this.embedded = false});

  final bool embedded;

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  late final TextEditingController _baseUrl;
  late final TextEditingController _authKey;
  late bool _allowUntrustedCerts;
  bool _obscureKey = true;
  bool _testing = false;
  String? _testResult;
  bool _testOk = false;

  AppState get _app => AppScope.of(context);

  bool _initialized = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (!_initialized) {
      _initialized = true;
      final settings = _app.settings;
      _baseUrl = TextEditingController(text: settings.baseUrl);
      _authKey = TextEditingController(text: settings.authKey);
      _allowUntrustedCerts = settings.allowUntrustedCerts;
    }
  }

  @override
  void dispose() {
    _baseUrl.dispose();
    _authKey.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    await _app.settings.updateConnection(
      baseUrl: _baseUrl.text,
      authKey: _authKey.text,
      allowUntrustedCerts: _allowUntrustedCerts,
    );
    if (mounted) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(S.of(context).settingsSaved)));
    }
  }

  Future<void> _test() async {
    setState(() {
      _testing = true;
      _testResult = null;
    });
    try {
      // Save first so the transport under test matches the form.
      await _app.settings.updateConnection(
        baseUrl: _baseUrl.text,
        authKey: _authKey.text,
        allowUntrustedCerts: _allowUntrustedCerts,
      );
      final result = await _app.client.listSessions();
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
      setState(() => _testing = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final settings = _app.settings;
    return Scaffold(
      appBar: AppBar(
        automaticallyImplyLeading: !widget.embedded,
        title: Text(S.of(context).settings),
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
                  Text(
                    S.of(context).daemonConnection,
                    style: theme.textTheme.titleSmall,
                  ),
                  const SizedBox(height: 4),
                  Text(
                    S.of(context).daemonConnectionDescription,
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                  const SizedBox(height: 12),
                  TextField(
                    controller: _baseUrl,
                    keyboardType: TextInputType.url,
                    decoration: InputDecoration(
                      labelText: S.of(context).daemonUrl,
                      hintText: 'http://127.0.0.1:8787',
                      border: const OutlineInputBorder(),
                      prefixIcon: const Icon(Icons.link_rounded),
                    ),
                  ),
                  const SizedBox(height: 12),
                  TextField(
                    controller: _authKey,
                    obscureText: _obscureKey,
                    decoration: InputDecoration(
                      labelText: S.of(context).authKey,
                      hintText: S.of(context).authKeyHint,
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
                    title: Text(S.of(context).allowSelfSigned),
                    subtitle: Text(S.of(context).allowSelfSignedHint),
                    value: _allowUntrustedCerts,
                    onChanged: (value) =>
                        setState(() => _allowUntrustedCerts = value),
                  ),
                  const SizedBox(height: 8),
                  Row(
                    children: [
                      FilledButton.icon(
                        onPressed: _save,
                        icon: const Icon(Icons.save_outlined, size: 18),
                        label: Text(S.of(context).save),
                      ),
                      const SizedBox(width: 12),
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
                        label: Text(S.of(context).testConnection),
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
                  const SizedBox(height: 24),
                  const Divider(),
                  const SizedBox(height: 12),
                  Text(
                    S.of(context).defaults,
                    style: theme.textTheme.titleSmall,
                  ),
                  const SizedBox(height: 8),
                  DropdownButtonFormField<String>(
                    initialValue: settings.appLanguage,
                    decoration: InputDecoration(
                      labelText: S.of(context).language,
                      border: const OutlineInputBorder(),
                    ),
                    items: [
                      DropdownMenuItem(
                        value: 'system',
                        child: Text(S.of(context).languageSystem),
                      ),
                      const DropdownMenuItem(
                        value: 'en',
                        child: Text('English'),
                      ),
                      const DropdownMenuItem(value: 'zh', child: Text('中文')),
                    ],
                    onChanged: (value) {
                      if (value != null) {
                        settings.setAppLanguage(value);
                      }
                    },
                  ),
                  const SizedBox(height: 12),
                  DropdownButtonFormField<String>(
                    initialValue: settings.defaultCapabilityMode,
                    decoration: InputDecoration(
                      labelText: S.of(context).defaultCapabilityMode,
                      border: const OutlineInputBorder(),
                    ),
                    items: [
                      for (final mode in CapabilityMode.all)
                        DropdownMenuItem(
                          value: mode,
                          child: Text(capabilityModeLabel(S.of(context), mode)),
                        ),
                    ],
                    onChanged: (value) {
                      if (value != null) {
                        settings.setDefaultCapabilityMode(value);
                      }
                    },
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}
