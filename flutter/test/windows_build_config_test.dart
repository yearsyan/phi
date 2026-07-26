import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

void main() {
  test('Windows runner uses standard C++20 CppWinRT coroutines', () {
    final cmakeFile = File('windows/runner/CMakeLists.txt');

    expect(cmakeFile.existsSync(), isTrue);
    final cmake = cmakeFile.readAsStringSync();
    expect(
      cmake,
      contains('target_compile_features(\${BINARY_NAME} PRIVATE cxx_std_20)'),
    );
    expect(cmake, contains('VS_GLOBAL_CppWinRTEnableLegacyCoroutines "false"'));
  });
}
