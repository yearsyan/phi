import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/app_licenses.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('bundled font and native Windows licenses are registered', () async {
    registerBundledAssetLicenses();

    final fontData = await rootBundle.load(notoSansScFontAsset);
    expect(fontData.lengthInBytes, 17772300);

    final licenseText = await rootBundle.loadString(notoSansScLicenseAsset);
    expect(licenseText, contains('SIL OPEN FONT LICENSE Version 1.1'));
    expect(licenseText, contains('Copyright 2014-2021 Adobe'));

    final entries = await LicenseRegistry.licenses
        .where((entry) => entry.packages.contains('Noto Sans SC'))
        .toList();
    expect(entries, hasLength(1));
    expect(
      entries.single.paragraphs.map((paragraph) => paragraph.text).join('\n'),
      contains('SIL OPEN FONT LICENSE Version 1.1'),
    );

    final windowsAppSdkLicense = await rootBundle.loadString(
      windowsAppSdkLicenseAsset,
    );
    expect(windowsAppSdkLicense, contains('MICROSOFT WINDOWS APP SDK'));
    expect(windowsAppSdkLicense, contains('3. DISTRIBUTABLE CODE.'));

    final cppWinRtLicense = await rootBundle.loadString(cppWinRtLicenseAsset);
    expect(cppWinRtLicense, contains('MIT License'));
    expect(cppWinRtLicense, contains('Copyright (c) Microsoft Corporation.'));

    final windowsEntries = await LicenseRegistry.licenses
        .where(
          (entry) => entry.packages.contains('Microsoft Windows App SDK 1.8'),
        )
        .toList();
    expect(windowsEntries, hasLength(1));

    final cppWinRtEntries = await LicenseRegistry.licenses
        .where(
          (entry) => entry.packages.contains(
            'Microsoft.Windows.CppWinRT 2.0.230706.1',
          ),
        )
        .toList();
    expect(cppWinRtEntries, hasLength(1));
  });
}
