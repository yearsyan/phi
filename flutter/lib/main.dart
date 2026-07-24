import 'package:flutter/material.dart';

import 'app.dart';
import 'app_licenses.dart';
import 'core/settings/app_settings.dart';
import 'state/app_state.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  registerBundledAssetLicenses();
  final settings = await AppSettings.load();
  runApp(PhiApp(appState: AppState(settings)));
}
