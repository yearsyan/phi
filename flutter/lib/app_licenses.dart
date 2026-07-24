import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

const notoSansScFontAsset = 'assets/fonts/noto_sans_sc/NotoSansSC-Variable.ttf';
const notoSansScLicenseAsset = 'assets/fonts/noto_sans_sc/OFL.txt';
const cppWinRtLicenseAsset = 'assets/licenses/cppwinrt.txt';
const windowsAppSdkLicenseAsset = 'assets/licenses/windows_app_sdk.txt';

/// Makes bundled font and native Windows dependency licenses visible.
void registerBundledAssetLicenses() {
  LicenseRegistry.addLicense(() async* {
    final notoLicense = await rootBundle.loadString(notoSansScLicenseAsset);
    yield LicenseEntryWithLineBreaks(const ['Noto Sans SC'], notoLicense);

    final windowsAppSdkLicense = await rootBundle.loadString(
      windowsAppSdkLicenseAsset,
    );
    yield LicenseEntryWithLineBreaks(const [
      'Microsoft Windows App SDK 1.8',
    ], windowsAppSdkLicense);

    final cppWinRtLicense = await rootBundle.loadString(cppWinRtLicenseAsset);
    yield LicenseEntryWithLineBreaks(const [
      'Microsoft.Windows.CppWinRT 2.0.230706.1',
    ], cppWinRtLicense);
  });
}
