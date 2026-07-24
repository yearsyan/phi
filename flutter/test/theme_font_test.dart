import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/ui/theme.dart';

void main() {
  test('light and dark themes use the bundled complete CJK font', () {
    for (final theme in [AppTheme.light(), AppTheme.dark()]) {
      expect(theme.textTheme.bodyMedium?.fontFamily, AppTheme.textFontFamily);
      expect(theme.textTheme.titleSmall?.fontFamily, AppTheme.textFontFamily);
    }
  });
}
