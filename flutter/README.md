# Phi Flutter client

A Flutter client for the phi coding-agent daemon, targeting Android, iOS,
macOS, Windows, and HarmonyOS/OpenHarmony. The shared Dart code is otherwise
platform-agnostic; Linux can use standard Flutter scaffolding.

## Features

- **Session management** — workspace-grouped session list (pinned first),
  filter, pin/unpin, delete, pull-to-refresh, live status dots.
- **Chat** — streaming assistant text with markdown + syntax highlighting,
  collapsible reasoning blocks, expandable tool-call rows (streaming args,
  progress lines, results, error states), per-turn activity summaries,
  compaction dividers, `askuser` question cards, tool-permission approval
  cards, queued prompts, stop,
  fork-from-reply, auto-generated titles, context-usage meter.
- **In-session controls** — capability mode, model, reasoning effort,
  `/compact`, user-invocable skills via the slash palette.
- **Workspaces** — recent workspaces + a directory browser backed by
  `GET /v1/workspaces/browse`.
- **Scheduled tasks** — list / create (daily or interval) / enable / run now /
  delete, open the session produced by the last run.
- **Adaptive layout** — phone: stacked navigation; wide screens (macOS,
  Windows, tablets): sidebar + detail pane.

Provider configuration is intentionally not included (managed via the web
client or `PUT /v1/providers`).

## Architecture

```
lib/
  core/
    transport/        # Pluggable transport layer
      daemon_transport.dart   # DaemonTransport / DaemonSocket interfaces
      direct_transport.dart   # Direct HTTP(S) + WS(WSS) implementation
    models/wire.dart  # Dart mirror of the daemon wire protocol (dto.rs)
    settings/         # SharedPreferences-backed settings
  state/
    daemon_client.dart        # Typed REST client over DaemonTransport
    session_controller.dart   # Per-session WS state machine (snapshot,
                              #   event reduction, reconnect/backoff, resync)
    sessions_store.dart       # Session list polling store
    app_state.dart            # Root state: settings → transport → client
  ui/                 # Pages (sessions, chat, tasks, machines, settings) + widgets
```

### Pluggable transports

All daemon access goes through `DaemonTransport` (REST-style `request` +
message-stream `connect`). Today only `DirectDaemonTransport` (plain
HTTP/HTTPS + WebSocket) exists. Future HTTP-over-SSH or HTTP-over-Tailscale
channels only need to implement the same two methods — including the
WebSocket-subprotocol auth mapping (`phi.v1`, `phi.auth.<token>`) — and can
be selected in settings without touching any UI or state code.

### Session connection lifecycle

Mirrors the web client: mint a single-use token via `POST /v1/auth/token`,
offer it as a WS subprotocol, then either `/v1/ws/new` (prepared session;
promoted to a real session by the first prompt) or `/v1/ws/attach/{id}`.
Strict sequence-gap detection triggers reconnect-with-resync; reconnect
backoff is 800/1600/3200/5000 ms. The reducer is a Dart port of the web
client's `sessionReducer`.

## Running

```sh
flutter pub get
# Windows release build
flutter build windows --release
# macOS
flutter run -d macos
# iOS unsigned release build (device install/archive still requires signing)
flutter build ios --release --no-codesign
# Android (daemon on the host's loopback):
adb reverse tcp:8787 tcp:8787
flutter run -d <android-device-id>
```

The app supports multiple daemon machines: each machine stores a name, the
daemon URL, the auth key (contents of `PHI_DAEMON_AUTH_KEY_FILE`) and its
own self-signed-certificate toggle. Manage them under **Settings →
Machines**, and switch the active machine from the sessions-page app bar
(one tap, no restart). On Android/iOS you can add a machine by scanning the
connection QR code that `phi-daemon` prints to its terminal at startup
(pass `--no-qr` to disable it): use the **Scan to connect** action on the
unconfigured sessions screen, or **Scan QR code** inside the machine
editor. Scanning fills in the URL and auth key only; it never changes the
self-signed certificate toggle. The scan entries are mobile-only and
require camera permission (declared in the Android manifest and iOS
`Info.plist`). For development, the first machine can also be seeded:

```sh
flutter run --dart-define=PHI_DAEMON_URL=http://127.0.0.1:8787 \
            --dart-define=PHI_DAEMON_KEY=<key>
```

### Windows

Windows builds require Windows 10 or 11 and Visual Studio 2022 with the
**Desktop development with C++** workload and a Windows SDK. Visual Studio Code
alone is not a Windows compiler toolchain. Confirm the installation and build
the x64 release bundle with:

```powershell
flutter doctor -v
flutter pub get
flutter build windows --release
```

The runnable bundle is written to
`build/windows/x64/runner/Release/`; keep the executable, DLLs, and `data/`
directory together when copying it. The Windows runner includes the same
multi-image attachment flow as Android, backed by the native file picker and
Windows Imaging Component. Selected images are oriented, resized, converted to
JPEG, and compressed before entering the daemon prompt. QR scanning remains
mobile-only.

On every GitHub push, `.github/workflows/build-windows-client-release.yml`
builds this release bundle with Flutter 3.44.6 and uploads the
`phi-client-windows-x64` Actions artifact.

### Android

Android release builds support ARM64 (`arm64-v8a`) only, use a dedicated
release key, and never fall back to the debug signing config. A local or CI
release build must provide all four environment variables:

- `ANDROID_RELEASE_KEYSTORE_PATH`
- `ANDROID_RELEASE_STORE_PASSWORD`
- `ANDROID_RELEASE_KEY_ALIAS`
- `ANDROID_RELEASE_KEY_PASSWORD`

Then build the signed APK with:

```sh
flutter build apk --release --target-platform android-arm64
```

On every GitHub push, `.github/workflows/build-android-release.yml` restores
the ignored keystore from `ANDROID_RELEASE_KEYSTORE_BASE64`, builds the signed
ARM64-only APK, verifies its packaged ABI and signature, and uploads
`phi-client-android-release.apk` to the Actions run. The repository must define
the base64 keystore secret plus the other three variables above as Actions
secrets. Never commit the keystore, passwords, generated APK, or a local
`key.properties`; keep a secure backup because future upgrades must use the
same signing identity.

### iOS

iOS builds require Xcode with an installed iOS Platform component and
CocoaPods. The committed runner uses bundle identifier `dev.phi.phiClient`
and targets iOS 13 or newer. A CI or local compile check can skip signing:

```sh
flutter build ios --release --no-codesign
```

To install on a physical device or create an archive/IPA, open
`ios/Runner.xcworkspace` in Xcode and select an Apple Development or
Distribution team and provisioning profile. Do not commit signing identities,
profiles, or certificate paths.

### HarmonyOS

The currently verified combination is:

- DevEco Studio 6.1.1 with HarmonyOS SDK API 24
- Flutter-OH `3.35.8-ohos-1.0.3-beta`
- An arm64 HarmonyOS 6.1 device (API 23)

Use `1.0.3-beta` specifically. The later 3.35 `1.0.4-beta` and 3.41 beta
currently reference autofill APIs that are not present in the released API 24
SDK.

The committed `pubspec.lock` is resolved with this Flutter-OH 3.35 / Dart 3.9
toolchain. Newer stable Flutter SDKs may rewrite SDK-pinned test dependencies;
do not commit that lockfile-only churn unless the Dart 3.9 compatibility floor
is intentionally removed.

Set up the Flutter-OH environment (adjust `FLUTTER_OHOS_HOME` if needed):

```sh
export FLUTTER_OHOS_HOME=/path/to/flutter-ohos-3.35
export DEVECO_SDK_HOME=/Applications/DevEco-Studio.app/Contents/sdk
export JAVA_HOME=/Applications/DevEco-Studio.app/Contents/jbr/Contents/Home
export PATH="$FLUTTER_OHOS_HOME/bin:/Applications/DevEco-Studio.app/Contents/tools/ohpm/bin:/Applications/DevEco-Studio.app/Contents/tools/hvigor/bin:/Applications/DevEco-Studio.app/Contents/tools/node/bin:/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/toolchains:$PATH"

flutter --version
./tool/ohos/patch_impellerc_macos_arm64.sh
```

On Apple Silicon, the Flutter-OH mirror currently supplies an Intel
`impellerc`. The patch script replaces only that executable. Do not replace
the entire `darwin-x64` artifact directory: its Flutter-OH snapshot files must
stay paired with `libflutter.so`, or the app will terminate with `Wrong full
snapshot version`.

On a new checkout, create the machine-local build profile and configure
automatic debug signing:

```sh
cp ohos/build-profile.template.json5 ohos/build-profile.json5
```

Open `ohos/` in DevEco Studio, then select **File → Project Structure →
Signing Configs → Automatically generate signature**. The resulting
`ohos/build-profile.json5` is ignored because it contains local certificate
paths and encrypted signing passwords.

Build, install, and start:

```sh
flutter build hap --debug
hdc list targets -v
hdc -t <device-id> install -r build/ohos/hap/entry-default-signed.hap
hdc -t <device-id> shell aa start \
  -a EntryAbility -b dev.phi.phi_client
```

## Tests

`test/daemon_smoke_test.dart` is an opt-in live-daemon smoke test: REST
list/browse plus a full WS new-session prompt round-trip, cleaning up after
itself. The default test suite does not contact a daemon. To enable the smoke
test explicitly:

```sh
flutter test

PHI_RUN_DAEMON_SMOKE_TEST=1 \
  PHI_DAEMON_AUTH_KEY_FILE="$HOME/.phi/daemon/auth.key" \
  PHI_DAEMON_URL=http://127.0.0.1:8787 \
  flutter test test/daemon_smoke_test.dart
```
