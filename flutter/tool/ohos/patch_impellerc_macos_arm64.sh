#!/bin/sh
set -eu

: "${FLUTTER_OHOS_HOME:?Set FLUTTER_OHOS_HOME to the Flutter-OH SDK directory}"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "This patch is only needed on Apple Silicon macOS."
  exit 0
fi

engine_version_file="$FLUTTER_OHOS_HOME/bin/internal/engine.version"
artifacts_dir="$FLUTTER_OHOS_HOME/bin/cache/artifacts/engine/darwin-x64"
impellerc_path="$artifacts_dir/impellerc"

if [ ! -f "$engine_version_file" ] || [ ! -f "$impellerc_path" ]; then
  echo "Initialize Flutter-OH first with: $FLUTTER_OHOS_HOME/bin/flutter --version" >&2
  exit 1
fi

if file "$impellerc_path" | grep -q 'arm64'; then
  echo "impellerc is already arm64."
  exit 0
fi

engine_version=$(tr -d '\r\n' < "$engine_version_file")
patch_tmp=$(mktemp -d "${TMPDIR:-/tmp}/flutter-ohos-impellerc.XXXXXX")
trap 'rm -rf "$patch_tmp"' EXIT HUP INT TERM

archive_url="https://storage.googleapis.com/flutter_infra_release/flutter/$engine_version/darwin-arm64/artifacts.zip"
curl -fL "$archive_url" -o "$patch_tmp/artifacts.zip"
unzip -q "$patch_tmp/artifacts.zip" impellerc -d "$patch_tmp"

if ! file "$patch_tmp/impellerc" | grep -q 'arm64'; then
  echo "Downloaded impellerc is not arm64." >&2
  exit 1
fi

if [ ! -e "$impellerc_path.x86_64.bak" ]; then
  cp -p "$impellerc_path" "$impellerc_path.x86_64.bak"
fi
install -m 755 "$patch_tmp/impellerc" "$impellerc_path"

echo "Patched only impellerc; Flutter-OH snapshot artifacts were preserved."
