#!/usr/bin/env bash
# Create the branded Minutes macOS installer DMG.
#
# Tauri's built-in create-dmg wrapper hides the useful stderr when the DMG
# cosmetics step fails on CI. Keep this script small and explicit so release
# logs show exactly which macOS packaging primitive failed.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/create-branded-dmg.sh --app <Minutes.app> --version <x.y.z> --output <path.dmg>

Creates a signed-app installer DMG with:
  - Minutes.app on the left
  - /Applications symlink on the right
  - the generated Minutes DMG background
EOF
}

APP_PATH=""
VERSION=""
OUTPUT_PATH=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app)
      APP_PATH="$2"
      shift 2
      ;;
    --version)
      VERSION="$2"
      shift 2
      ;;
    --output)
      OUTPUT_PATH="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$APP_PATH" || -z "$VERSION" || -z "$OUTPUT_PATH" ]]; then
  usage >&2
  exit 2
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "create-branded-dmg.sh requires macOS." >&2
  exit 1
fi

if [[ ! -d "$APP_PATH" ]]; then
  echo "App bundle not found: $APP_PATH" >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKGROUND_PATH="$REPO_ROOT/tauri/src-tauri/dmg-background.png"
ICON_PATH="$REPO_ROOT/tauri/src-tauri/icons/icon.icns"

if [[ ! -f "$BACKGROUND_PATH" ]]; then
  echo "DMG background not found: $BACKGROUND_PATH" >&2
  exit 1
fi

if [[ ! -f "$ICON_PATH" ]]; then
  echo "Volume icon not found: $ICON_PATH" >&2
  exit 1
fi

OUTPUT_DIR="$(dirname "$OUTPUT_PATH")"
mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd)"
OUTPUT_NAME="$(basename "$OUTPUT_PATH")"
FINAL_DMG="$OUTPUT_DIR/$OUTPUT_NAME"

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/minutes-dmg.XXXXXX")"
STAGING_DIR="$WORK_DIR/staging"
MOUNT_DIR="$WORK_DIR/mount"
RW_DMG="$WORK_DIR/Minutes-${VERSION}.rw.dmg"

cleanup() {
  if mount | grep -Fq "$MOUNT_DIR"; then
    hdiutil detach "$MOUNT_DIR" -force -quiet || true
  fi
  rm -rf "$WORK_DIR" || true
}
trap cleanup EXIT

echo "Preparing branded DMG staging folder..."
mkdir -p "$STAGING_DIR/.background" "$MOUNT_DIR"
cp -R "$APP_PATH" "$STAGING_DIR/Minutes.app"
ln -s /Applications "$STAGING_DIR/Applications"
cp "$BACKGROUND_PATH" "$STAGING_DIR/.background/dmg-background.png"
cp "$ICON_PATH" "$STAGING_DIR/.VolumeIcon.icns"

# The app bundle is roughly 125MB at release time. Leave generous headroom so
# Finder can write .DS_Store and hdiutil can resize/compress without ENOSPC.
APP_SIZE_MB="$(du -sm "$STAGING_DIR" | awk '{print $1}')"
DMG_SIZE_MB="$((APP_SIZE_MB + 80))"

echo "Creating read/write DMG (${DMG_SIZE_MB} MB)..."
rm -f "$RW_DMG" "$FINAL_DMG"
hdiutil create \
  -volname "Minutes" \
  -srcfolder "$STAGING_DIR" \
  -fs HFS+ \
  -fsargs "-c c=64,a=16,e=16" \
  -format UDRW \
  -size "${DMG_SIZE_MB}m" \
  "$RW_DMG"

echo "Mounting DMG..."
hdiutil attach "$RW_DMG" \
  -mountpoint "$MOUNT_DIR" \
  -readwrite \
  -noverify \
  -noautoopen

echo "Applying Finder window layout..."
if command -v SetFile >/dev/null 2>&1; then
  SetFile -a V "$MOUNT_DIR/.background" || true
  SetFile -a C "$MOUNT_DIR/.VolumeIcon.icns" || true
  SetFile -a C "$MOUNT_DIR" || true
fi
chflags hidden "$MOUNT_DIR/.background" "$MOUNT_DIR/.VolumeIcon.icns" || true

osascript - "$MOUNT_DIR" <<'APPLESCRIPT'
on run argv
  set mountPath to item 1 of argv
  set dmgFolder to POSIX file mountPath as alias
  set bgPic to POSIX file (mountPath & "/.background/dmg-background.png") as alias

  tell application "Finder"
    open dmgFolder
    set win to container window of dmgFolder
    set current view of win to icon view
    set toolbar visible of win to false
    set statusbar visible of win to false
    set bounds of win to {100, 100, 760, 500}

    set opts to the icon view options of win
    set arrangement of opts to not arranged
    set icon size of opts to 128
    set text size of opts to 16
    set background picture of opts to bgPic

    set position of item "Minutes.app" of dmgFolder to {180, 200}
    set position of item "Applications" of dmgFolder to {480, 200}
    update dmgFolder without registering applications
    delay 2
    close win
  end tell
end run
APPLESCRIPT

echo "Flushing Finder metadata..."
sync
sleep 2

echo "Detaching DMG..."
osascript -e 'tell application "Finder" to quit' >/dev/null 2>&1 || true

DETACHED=0
for attempt in 1 2 3 4 5; do
  if hdiutil detach "$MOUNT_DIR" -quiet; then
    DETACHED=1
    break
  fi

  echo "DMG detach attempt ${attempt} failed; retrying with force after Finder settles..." >&2
  diskutil unmount force "$MOUNT_DIR" >/dev/null 2>&1 || true
  if hdiutil detach "$MOUNT_DIR" -force -quiet; then
    DETACHED=1
    break
  fi
  sleep "$((attempt * 2))"
done

if [[ "$DETACHED" -ne 1 ]]; then
  echo "Failed to detach DMG mount after retries: $MOUNT_DIR" >&2
  exit 1
fi

echo "Compressing final DMG..."
hdiutil convert "$RW_DMG" \
  -format UDZO \
  -imagekey zlib-level=9 \
  -o "$FINAL_DMG"

echo "Branded DMG created: $FINAL_DMG"
