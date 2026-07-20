import 'package:flutter/material.dart';

/// App themes: deliberately understated "business app" styling in the spirit
/// of iOS 12–16 — flat surfaces, hairline separators, restrained radii and a
/// single accent color. No Material surface tints, tonal elevation or blur.
///
/// Layout is untouched; only look & feel.
class AppTheme {
  AppTheme._();

  /* ------------------------------ palette -------------------------------- */

  static const lightAccent = Color(0xFF007AFF);
  static const darkAccent = Color(0xFF0A84FF);

  // Light (iOS system colors)
  static const lightBackground = Color(0xFFFFFFFF);
  static const lightGrouped = Color(0xFFF2F2F7);
  static const lightFill = Color(0xFFE9E9EB);
  static const lightSeparator = Color(0xFFC6C6C8);
  static const lightLabel = Color(0xFF1C1C1E);
  static const lightSecondary = Color(0xFF8E8E93);
  static const lightError = Color(0xFFFF3B30);

  // Dark (iOS system colors)
  static const darkBackground = Color(0xFF000000);
  static const darkGrouped = Color(0xFF1C1C1E);
  static const darkFill = Color(0xFF2C2C2E);
  static const darkSeparator = Color(0xFF38383A);
  static const darkLabel = Color(0xFFF2F2F7);
  static const darkSecondary = Color(0xFF98989F);
  static const darkError = Color(0xFFFF453A);

  static const _radius = 10.0;

  static ThemeData light() => _build(
    brightness: Brightness.light,
    accent: lightAccent,
    background: lightBackground,
    grouped: lightGrouped,
    fill: lightFill,
    separator: lightSeparator,
    label: lightLabel,
    secondary: lightSecondary,
    error: lightError,
  );

  static ThemeData dark() => _build(
    brightness: Brightness.dark,
    accent: darkAccent,
    background: darkBackground,
    grouped: darkGrouped,
    fill: darkFill,
    separator: darkSeparator,
    label: darkLabel,
    secondary: darkSecondary,
    error: darkError,
  );

  static ThemeData _build({
    required Brightness brightness,
    required Color accent,
    required Color background,
    required Color grouped,
    required Color fill,
    required Color separator,
    required Color label,
    required Color secondary,
    required Color error,
  }) {
    final isDark = brightness == Brightness.dark;
    final hairline = BorderSide(color: separator, width: 0.5);

    final scheme = ColorScheme(
      brightness: brightness,
      primary: accent,
      onPrimary: Colors.white,
      secondary: accent,
      onSecondary: Colors.white,
      error: error,
      onError: Colors.white,
      surface: background,
      onSurface: label,
      surfaceContainerHighest: fill,
      surfaceContainerHigh: grouped,
      surfaceContainer: grouped,
      surfaceContainerLow: background,
      surfaceContainerLowest: background,
      primaryContainer: fill,
      onPrimaryContainer: label,
      secondaryContainer: fill,
      onSecondaryContainer: label,
      tertiaryContainer: fill,
      errorContainer: isDark
          ? const Color(0xFF3B1C1A)
          : const Color(0xFFFFE5E3),
      onErrorContainer: isDark
          ? const Color(0xFFFFD2CD)
          : const Color(0xFF8A1F18),
      outline: secondary,
      outlineVariant: separator,
      inverseSurface: isDark ? lightBackground : darkGrouped,
      onInverseSurface: isDark ? lightLabel : darkLabel,
      surfaceTint: Colors.transparent,
      shadow: Colors.transparent,
    );

    final textTheme =
        (isDark ? Typography.whiteMountainView : Typography.blackMountainView)
            .apply(
              bodyColor: label,
              displayColor: label,
              fontFamilyFallback: const ['.AppleSystemUIFont', 'PingFang SC'],
            );

    final flatButtonShape = RoundedRectangleBorder(
      borderRadius: BorderRadius.circular(_radius),
    );

    return ThemeData(
      useMaterial3: true,
      brightness: brightness,
      colorScheme: scheme,
      scaffoldBackgroundColor: background,
      textTheme: textTheme,
      splashFactory: InkSparkle.constantTurbulenceSeedSplashFactory,
      highlightColor: fill.withAlpha(80),
      hoverColor: fill.withAlpha(60),

      appBarTheme: AppBarTheme(
        backgroundColor: background,
        foregroundColor: label,
        elevation: 0,
        scrolledUnderElevation: 0,
        centerTitle: false,
        titleSpacing: 16,
        shape: Border(bottom: hairline),
        iconTheme: IconThemeData(color: label, size: 22),
        actionsIconTheme: IconThemeData(color: label, size: 22),
      ),

      dividerTheme: DividerThemeData(
        color: separator,
        thickness: 0.5,
        space: 1,
      ),

      cardTheme: CardThemeData(
        color: isDark ? grouped : background,
        elevation: 0,
        margin: EdgeInsets.zero,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(_radius),
          side: hairline,
        ),
      ),

      dialogTheme: DialogThemeData(
        backgroundColor: isDark ? grouped : background,
        elevation: 0,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(14),
          side: isDark ? hairline : BorderSide.none,
        ),
        titleTextStyle: textTheme.titleMedium?.copyWith(color: label),
      ),

      bottomSheetTheme: BottomSheetThemeData(
        backgroundColor: isDark ? grouped : background,
        elevation: 0,
        modalElevation: 0,
        shape: const RoundedRectangleBorder(
          borderRadius: BorderRadius.vertical(top: Radius.circular(14)),
        ),
      ),

      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          backgroundColor: accent,
          foregroundColor: Colors.white,
          disabledBackgroundColor: fill,
          disabledForegroundColor: secondary,
          shape: flatButtonShape,
          padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 12),
          textStyle: const TextStyle(fontSize: 15, fontWeight: FontWeight.w500),
        ),
      ),

      textButtonTheme: TextButtonThemeData(
        style: TextButton.styleFrom(
          foregroundColor: accent,
          shape: flatButtonShape,
          textStyle: const TextStyle(fontSize: 15, fontWeight: FontWeight.w400),
        ),
      ),

      outlinedButtonTheme: OutlinedButtonThemeData(
        style: OutlinedButton.styleFrom(
          foregroundColor: accent,
          side: BorderSide(color: accent.withAlpha(160)),
          shape: flatButtonShape,
          padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 12),
        ),
      ),

      iconButtonTheme: IconButtonThemeData(
        style: IconButton.styleFrom(foregroundColor: label),
      ),

      floatingActionButtonTheme: FloatingActionButtonThemeData(
        backgroundColor: accent,
        foregroundColor: Colors.white,
        elevation: 0,
        highlightElevation: 0,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
      ),

      chipTheme: ChipThemeData(
        backgroundColor: Colors.transparent,
        selectedColor: fill,
        disabledColor: fill,
        side: hairline,
        labelStyle: TextStyle(color: label, fontSize: 12),
        secondaryLabelStyle: TextStyle(color: secondary, fontSize: 12),
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      ),

      inputDecorationTheme: InputDecorationTheme(
        filled: false,
        isDense: true,
        hintStyle: TextStyle(color: secondary),
        labelStyle: TextStyle(color: secondary),
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(_radius),
          borderSide: hairline,
        ),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(_radius),
          borderSide: hairline,
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(_radius),
          borderSide: BorderSide(color: accent, width: 1),
        ),
      ),

      popupMenuTheme: PopupMenuThemeData(
        color: isDark ? grouped : background,
        elevation: 2,
        shadowColor: Colors.black.withAlpha(40),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(_radius),
          side: isDark ? hairline : BorderSide.none,
        ),
        labelTextStyle: WidgetStatePropertyAll(
          TextStyle(color: label, fontSize: 14),
        ),
      ),

      listTileTheme: ListTileThemeData(
        iconColor: secondary,
        textColor: label,
        dense: false,
      ),

      switchTheme: SwitchThemeData(
        thumbColor: const WidgetStatePropertyAll(Colors.white),
        trackColor: WidgetStateProperty.resolveWith((states) {
          if (states.contains(WidgetState.selected)) {
            return const Color(0xFF34C759); // iOS green
          }
          return isDark ? const Color(0xFF39393D) : const Color(0xFFE9E9EA);
        }),
        trackOutlineColor: const WidgetStatePropertyAll(Colors.transparent),
      ),

      snackBarTheme: SnackBarThemeData(
        behavior: SnackBarBehavior.floating,
        backgroundColor: isDark ? darkFill : darkGrouped,
        contentTextStyle: TextStyle(color: isDark ? darkLabel : darkLabel),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(_radius),
        ),
      ),

      progressIndicatorTheme: ProgressIndicatorThemeData(
        color: accent,
        linearTrackColor: fill,
        circularTrackColor: fill,
      ),

      segmentedButtonTheme: SegmentedButtonThemeData(
        style: ButtonStyle(
          foregroundColor: WidgetStateProperty.resolveWith((states) {
            if (states.contains(WidgetState.selected)) return label;
            return secondary;
          }),
          backgroundColor: WidgetStateProperty.resolveWith((states) {
            if (states.contains(WidgetState.selected)) return fill;
            return Colors.transparent;
          }),
          side: WidgetStatePropertyAll(hairline),
          shape: WidgetStatePropertyAll(
            RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
          ),
        ),
      ),

      tabBarTheme: TabBarThemeData(
        labelColor: accent,
        unselectedLabelColor: secondary,
        indicatorColor: accent,
        dividerColor: Colors.transparent,
      ),

      tooltipTheme: TooltipThemeData(
        decoration: BoxDecoration(
          color: isDark ? darkFill : darkGrouped,
          borderRadius: BorderRadius.circular(8),
        ),
        textStyle: TextStyle(
          color: isDark ? darkLabel : darkLabel,
          fontSize: 12,
        ),
      ),
    );
  }
}
