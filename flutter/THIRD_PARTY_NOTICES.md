# Third-party notices

## Noto Sans SC

Phi bundles the unmodified Noto Sans SC variable font distributed by the
official Google Fonts repository.

- Copyright: 2014–2021 Adobe, with Reserved Font Name "Source"
- License: SIL Open Font License, Version 1.1
- Google Fonts revision:
  `2894aab31764f10f29c421bdfd2340d3b382d384`
- Source:
  <https://github.com/google/fonts/tree/2894aab31764f10f29c421bdfd2340d3b382d384/ofl/notosanssc>
- Bundled font:
  `assets/fonts/noto_sans_sc/NotoSansSC-Variable.ttf`
- SHA-256:
  `a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da`

The complete license text and copyright notice are preserved at
[`assets/fonts/noto_sans_sc/OFL.txt`](assets/fonts/noto_sans_sc/OFL.txt). The
same text is registered with Flutter's `LicenseRegistry` and is available in
the app under **Settings → About → Open-source licenses**.

## Microsoft Windows App SDK

The Windows build uses and self-contains the following official Microsoft
NuGet components:

- `Microsoft.WindowsAppSDK.InteractiveExperiences` 1.8.260125001
- `Microsoft.WindowsAppSDK.Base` 1.8.251216001 (transitive)

These components provide `AppWindowTitleBar`, the native title-bar overlay, and
their required runtime binaries. They are redistributed under the Microsoft
Windows App SDK Software License Terms supplied with the NuGet packages. The
complete terms are bundled at
[`assets/licenses/windows_app_sdk.txt`](assets/licenses/windows_app_sdk.txt)
and registered with Flutter's `LicenseRegistry`.

Sources:

- <https://www.nuget.org/packages/Microsoft.WindowsAppSDK.InteractiveExperiences/1.8.260125001>
- <https://www.nuget.org/packages/Microsoft.WindowsAppSDK.Base/1.8.251216001>

## Microsoft.Windows.CppWinRT

The Windows build uses `Microsoft.Windows.CppWinRT` 2.0.230706.1 to generate
the standard C++ projection for Windows Runtime APIs. Its headers and tooling
are licensed under the MIT License, Copyright (c) Microsoft Corporation.

The complete license is bundled at
[`assets/licenses/cppwinrt.txt`](assets/licenses/cppwinrt.txt) and registered
with Flutter's `LicenseRegistry`.

Source:
<https://www.nuget.org/packages/Microsoft.Windows.CppWinRT/2.0.230706.1>
