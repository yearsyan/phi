import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';

import 'state/app_state.dart';
import 'ui/home_shell.dart';
import 'ui/theme.dart';

/// Root widget: injects [AppState] and builds the adaptive shell.
class PhiApp extends StatelessWidget {
  const PhiApp({super.key, required this.appState});

  final AppState appState;

  @override
  Widget build(BuildContext context) {
    // AppScope sits above MaterialApp so pushed routes (chat, settings,
    // scheduled tasks) can still reach it through the root navigator.
    return AppScope(
      state: appState,
      child: ListenableBuilder(
        listenable: appState.settings,
        builder: (context, _) {
          final language = appState.settings.appLanguage;
          final localeOverride = switch (language) {
            'en' => const Locale('en'),
            'zh' => const Locale('zh'),
            _ => null, // system
          };
          return MaterialApp(
            title: 'Phi',
            debugShowCheckedModeBanner: false,
            themeMode: ThemeMode.system,
            locale: localeOverride,
            supportedLocales: const [Locale('en'), Locale('zh')],
            localizationsDelegates: const [
              GlobalMaterialLocalizations.delegate,
              GlobalWidgetsLocalizations.delegate,
              GlobalCupertinoLocalizations.delegate,
            ],
            theme: AppTheme.light(),
            darkTheme: AppTheme.dark(),
            home: const HomeShell(),
          );
        },
      ),
    );
  }
}

/// Provides [AppState] down the tree.
class AppScope extends InheritedWidget {
  const AppScope({super.key, required this.state, required super.child});

  final AppState state;

  static AppState of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<AppScope>()!.state;

  @override
  bool updateShouldNotify(AppScope oldWidget) =>
      !identical(state, oldWidget.state);
}
