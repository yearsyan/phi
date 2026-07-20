import 'package:flutter/material.dart';

import 'app.dart';
import 'core/settings/app_settings.dart';
import 'state/app_state.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final settings = await AppSettings.load();
  runApp(PhiApp(appState: AppState(settings)));
}
