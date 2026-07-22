#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Bind installed acceptance results to the exact checked-out source commit.
# build.rs embeds this value in the desktop diagnostic payload, while the
# acceptance runner independently compares the installed executable with the
# freshly signed bundle produced below.
export MINUTES_BUILD_COMMIT="$(git rev-parse --verify HEAD)"

export CXXFLAGS="${CXXFLAGS:-"-I$(xcrun --show-sdk-path)/usr/include/c++/v1"}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
# engine-sherpa / vad-ort stay OPT-IN for app builds pending the #369 release
# decision. Packaging is fixed for opt-in local builds: sherpa links statically
# and the existing ort path does not leave dangling runtime dylib references.
# Opt in explicitly:
#   MINUTES_BUILD_FEATURES=parakeet,metal,vad-ort,engine-sherpa
if [ -z "${MINUTES_BUILD_FEATURES+x}" ]; then
    MINUTES_BUILD_FEATURES="parakeet,metal"
fi

# Match scripts/build.sh: route cargo through rustup so rust-toolchain.toml
# is honored and we don't drift from CI's clippy/rustfmt versions. Uses
# `rustup which cargo` so CARGO_HOME / non-default rustup paths still work.
RUSTUP_CARGO=""
if command -v rustup >/dev/null 2>&1; then
    RUSTUP_CARGO="$(rustup which cargo 2>/dev/null || true)"
fi
if [[ -n "$RUSTUP_CARGO" ]]; then
    export PATH="$(dirname "$RUSTUP_CARGO"):$PATH"
fi
ACTIVE_CARGO="$(command -v cargo || true)"
if [[ -z "$ACTIVE_CARGO" ]]; then
    echo "Error: no cargo on PATH. Install rustup from https://rustup.rs and re-run."
    exit 1
fi
if [[ -n "$RUSTUP_CARGO" && "$ACTIVE_CARGO" != "$RUSTUP_CARGO" ]]; then
    echo "Warning: cargo at $ACTIVE_CARGO is not the rustup-managed cargo ($RUSTUP_CARGO); rust-toolchain.toml may be ignored."
fi

DEV_CONFIG="tauri/src-tauri/tauri.dev.conf.json"
DEV_PRODUCT_NAME="Minutes Dev"
BUILD_APP="target/release/bundle/macos/${DEV_PRODUCT_NAME}.app"
INSTALL_DIR="${INSTALL_DIR:-$HOME/Applications}"
INSTALL_APP="${INSTALL_DIR}/${DEV_PRODUCT_NAME}.app"
SIGNING_IDENTITY="${MINUTES_DEV_SIGNING_IDENTITY:-${APPLE_SIGNING_IDENTITY:-}}"
SIGN_MODE="adhoc"
LOCK_DIR="${INSTALL_DIR}/.${DEV_PRODUCT_NAME}.install-lock"
STAGED_APP="${INSTALL_DIR}/.${DEV_PRODUCT_NAME}.staged-$$.app"
BACKUP_APP="${INSTALL_DIR}/.${DEV_PRODUCT_NAME}.backup-$$.app"
INSTALL_LOCK_HELD=0
INSTALL_SWAP_ACTIVE=0
HELPER_BACKUP_DIR=""
HELPER_WORKSPACE_PREPARED=0
TRACKED_BUILD_HELPERS=(
  "tauri/src-tauri/bin/mic_check"
  "tauri/src-tauri/bin/mic_check-aarch64-apple-darwin"
)

assert_clean_build_source() {
  local dirty
  dirty="$(git status --porcelain=v1 --untracked-files=all)"
  if [[ -n "$dirty" ]]; then
    echo "Refusing to build Minutes Dev from uncommitted application or harness source:" >&2
    printf '%s\n' "$dirty" >&2
    return 1
  fi
}

write_head_helper() {
  local helper="$1"
  local staged
  staged="$(mktemp "${TMPDIR:-/tmp}/minutes-dev-head-helper.XXXXXX")"
  git show "HEAD:${helper}" > "$staged"
  chmod 755 "$staged"
  /bin/mv -f "$staged" "$helper"
}

prepare_canonical_build_helpers() {
  local index
  local helper
  HELPER_BACKUP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/minutes-dev-helper-backup.XXXXXX")"
  for index in "${!TRACKED_BUILD_HELPERS[@]}"; do
    helper="${TRACKED_BUILD_HELPERS[$index]}"
    if [[ -e "$helper" ]]; then
      cp -p "$helper" "$HELPER_BACKUP_DIR/$index"
    else
      : > "$HELPER_BACKUP_DIR/$index.missing"
    fi
  done
  HELPER_WORKSPACE_PREPARED=1
  for helper in "${TRACKED_BUILD_HELPERS[@]}"; do
    write_head_helper "$helper"
  done
}

reset_tracked_build_helpers_to_head() {
  local helper
  for helper in "${TRACKED_BUILD_HELPERS[@]}"; do
    write_head_helper "$helper"
  done
}

remove_generated_build_helpers() {
  rm -f \
    tauri/src-tauri/bin/mic_check \
    tauri/src-tauri/bin/mic_check-* \
    tauri/src-tauri/bin/system_audio_record \
    tauri/src-tauri/bin/system_audio_record-* \
    tauri/src-tauri/bin/calendar-events \
    tauri/src-tauri/bin/calendar-events-*
}

restore_user_build_helpers() {
  local index
  local helper
  if [[ "$HELPER_WORKSPACE_PREPARED" != "1" ]]; then
    return
  fi
  for index in "${!TRACKED_BUILD_HELPERS[@]}"; do
    helper="${TRACKED_BUILD_HELPERS[$index]}"
    if [[ -f "$HELPER_BACKUP_DIR/$index.missing" ]]; then
      rm -f "$helper"
    else
      cp -p "$HELPER_BACKUP_DIR/$index" "$helper"
    fi
  done
  HELPER_WORKSPACE_PREPARED=0
  rm -rf "$HELPER_BACKUP_DIR"
  HELPER_BACKUP_DIR=""
}

cleanup_install_artifacts() {
  if [[ "$INSTALL_SWAP_ACTIVE" == "1" && -d "$BACKUP_APP" ]]; then
    if [[ ! -d "$INSTALL_APP" || -z "$(running_dev_bundle_pids)" ]]; then
      rm -rf "$INSTALL_APP"
      /bin/mv -f "$BACKUP_APP" "$INSTALL_APP"
      echo "Interrupted install: restored the previous ${DEV_PRODUCT_NAME}.app." >&2
      INSTALL_SWAP_ACTIVE=0
    else
      echo "Interrupted install: the replacement is running; previous app preserved at $BACKUP_APP" >&2
    fi
  fi
  rm -rf "$STAGED_APP"
  if [[ "$INSTALL_LOCK_HELD" == "1" ]]; then
    rm -rf "$LOCK_DIR"
  fi
  restore_user_build_helpers
}

acquire_install_lock() {
  mkdir -p "$INSTALL_DIR"
  if ! mkdir "$LOCK_DIR" 2>/dev/null; then
    echo "Another ${DEV_PRODUCT_NAME} install is already running (lock: $LOCK_DIR)." >&2
    exit 1
  fi
  INSTALL_LOCK_HELD=1
  printf '%s\n' "$$" > "$LOCK_DIR/pid"
}

bundle_process_rows() {
  local marker="$INSTALL_APP/Contents/MacOS/"
  local pid
  local executable
  ps -ww -axo pid=,command= | awk -v marker="$marker" 'index($0, marker) { print $1 }' | while read -r pid; do
    [[ -n "$pid" ]] || continue
    executable="$(/usr/sbin/lsof -a -p "$pid" -d txt -Fn 2>/dev/null \
      | sed -n 's/^n//p' \
      | awk -v marker="$marker" 'index($0, marker) == 1 { print; exit }')"
    case "$executable" in
      "$marker"*) printf '%s\t%s\n' "$pid" "$executable" ;;
    esac
  done
}

running_dev_app_pids() {
  local executable="$INSTALL_APP/Contents/MacOS/minutes-app"
  bundle_process_rows | awk -F '\t' -v executable="$executable" '$2 == executable { print $1 }'
}

running_dev_bundle_pids() {
  bundle_process_rows | awk -F '\t' '{ print $1 }'
}

stop_running_dev_app() {
  local force_failed_candidate="${1:-0}"
  local pids
  pids="$(running_dev_bundle_pids)"
  if [[ -z "$pids" ]]; then
    return
  fi

  echo "=== Closing the running ${DEV_PRODUCT_NAME}.app before replacement ==="
  if [[ -n "$(running_dev_app_pids)" ]] && ! osascript \
    -e 'with timeout of 5 seconds' \
    -e 'tell application id "com.useminutes.desktop.dev" to quit' \
    -e 'end timeout'; then
    if [[ "$force_failed_candidate" != "1" ]]; then
      echo "Could not ask ${DEV_PRODUCT_NAME}.app to quit safely. Quit it manually and rerun the installer." >&2
      return 1
    fi
    echo "The failed candidate did not accept a quit request; retiring only its verified bundle process." >&2
  fi
  local attempt
  local graceful_attempts=40
  if [[ "$force_failed_candidate" == "1" ]]; then
    graceful_attempts=10
  fi
  for ((attempt = 0; attempt < graceful_attempts; attempt++)); do
    if [[ -z "$(running_dev_bundle_pids)" ]]; then
      return
    fi
    sleep 0.5
  done

  if [[ "$force_failed_candidate" == "1" ]]; then
    pids="$(running_dev_bundle_pids)"
    if [[ -n "$pids" ]]; then
      echo "Terminating failed candidate bundle PID(s): $(printf '%s' "$pids" | tr '\n' ' ')" >&2
      kill -TERM $pids 2>/dev/null || true
      for ((attempt = 0; attempt < 10; attempt++)); do
        if [[ -z "$(running_dev_bundle_pids)" ]]; then
          return
        fi
        sleep 0.5
      done
    fi

    pids="$(running_dev_bundle_pids)"
    if [[ -n "$pids" ]]; then
      echo "Force-stopping unresponsive failed candidate bundle PID(s): $(printf '%s' "$pids" | tr '\n' ' ')" >&2
      kill -KILL $pids 2>/dev/null || true
      for ((attempt = 0; attempt < 20; attempt++)); do
        if [[ -z "$(running_dev_bundle_pids)" ]]; then
          return
        fi
        sleep 0.25
      done
    fi
  fi

  echo "${DEV_PRODUCT_NAME}.app did not exit within 20 seconds; refusing to replace a running bundle." >&2
  echo "Still running bundle PID(s): $(running_dev_bundle_pids | tr '\n' ' ')" >&2
  return 1
}

restore_previous_app() {
  local relaunch="${1:-0}"
  local force_failed_candidate="${2:-0}"

  if [[ -n "$(running_dev_bundle_pids)" ]] && ! stop_running_dev_app "$force_failed_candidate"; then
    echo "Could not stop the failed candidate; previous app remains at $BACKUP_APP" >&2
    return 1
  fi

  rm -rf "$INSTALL_APP"
  if [[ ! -d "$BACKUP_APP" ]]; then
    echo "No previous app existed; removed the failed candidate." >&2
    return 0
  fi

  /bin/mv -f "$BACKUP_APP" "$INSTALL_APP"
  INSTALL_SWAP_ACTIVE=0
  echo "Previous app restored." >&2
  if [[ "$relaunch" == "1" ]]; then
    echo "Relaunching the previous app." >&2
    if ! open -n "$INSTALL_APP"; then
      echo "The previous app was restored but could not be relaunched automatically." >&2
      return 1
    fi
  fi
}

verify_frontend_startup() {
  local launch_started_unix_ms="$1"
  local status_path="$HOME/.minutes/desktop-control/desktop-app-com.useminutes.desktop.dev.json"
  local pids=""
  local pid=""
  local status_pid=""
  local frontend_ready=""
  local frontend_error=""
  local process_started_unix_ms=""
  local frontend_ready_at_unix_ms=""
  # LaunchServices can spend more than 30 seconds registering a freshly
  # replaced, freshly signed bundle before the process creates its WebView.
  # Keep the rollback gate strict, but give a cold launch enough time to
  # produce the same PID-bound heartbeat that warm launches emit quickly.
  local max_attempts=180
  local attempt

  echo "=== Verifying fresh desktop frontend startup ==="
  for ((attempt = 0; attempt < max_attempts; attempt++)); do
    pids="$(running_dev_app_pids)"
    if [[ -n "$pids" && "$(printf '%s\n' "$pids" | wc -l | tr -d ' ')" == "1" ]]; then
      pid="$pids"
      if [[ -f "$status_path" ]]; then
        status_pid="$(plutil -extract pid raw -o - "$status_path" 2>/dev/null || true)"
        frontend_ready="$(plutil -extract frontend_ready raw -o - "$status_path" 2>/dev/null || true)"
        frontend_error="$(plutil -extract frontend_error raw -o - "$status_path" 2>/dev/null || true)"
        process_started_unix_ms="$(plutil -extract process_started_at_unix_ms raw -o - "$status_path" 2>/dev/null || true)"
        frontend_ready_at_unix_ms="$(plutil -extract frontend_ready_at_unix_ms raw -o - "$status_path" 2>/dev/null || true)"
        if [[ "$status_pid" == "$pid" && "$frontend_ready" == "true" \
          && "$process_started_unix_ms" =~ ^[0-9]+$ \
          && "$frontend_ready_at_unix_ms" =~ ^[0-9]+$ \
          && "$process_started_unix_ms" -ge "$launch_started_unix_ms" \
          && "$frontend_ready_at_unix_ms" -ge "$launch_started_unix_ms" ]]; then
          echo "  Frontend ready (fresh PID $pid)"
          return 0
        fi
        if [[ "$status_pid" == "$pid" && -n "$frontend_error" ]]; then
          echo "Frontend startup failed in fresh PID $pid:" >&2
          echo "$frontend_error" >&2
          return 1
        fi
      fi
    fi
    sleep 0.5
  done

  echo "Fresh ${DEV_PRODUCT_NAME}.app did not report a ready frontend within 90 seconds." >&2
  if [[ -n "$pids" ]]; then
    echo "Observed PID(s): $(printf '%s' "$pids" | tr '\n' ' ')" >&2
  else
    echo "No installed dev-app process is running." >&2
  fi
  if [[ -f "$status_path" ]]; then
    echo "Last desktop heartbeat:" >&2
    plutil -convert json -o - "$status_path" 2>/dev/null >&2 || true
  fi
  return 1
}

run_with_ort_retry() {
  local _build_tmp
  _build_tmp=$(mktemp)
  if ! "$@" 2>&1 | tee "$_build_tmp"; then
    if grep -q "library 'clang_rt\." "$_build_tmp"; then
      echo ""
      echo "  Stale ort-sys clang runtime path (Xcode/CLT upgrade detected)."
      echo "  Cleaning stale build cache and retrying..."
      rm -rf target/*/build/ort-sys-*
      rm -f "$_build_tmp"
      "$@"
      return
    fi
    rm -f "$_build_tmp"
    return 1
  fi
  rm -f "$_build_tmp"
}

OPEN_AFTER_INSTALL=1
INSTALL_AFTER_BUILD=1
for arg in "$@"; do
  case "$arg" in
    --no-open)
      OPEN_AFTER_INSTALL=0
      ;;
    --target-only)
      OPEN_AFTER_INSTALL=0
      INSTALL_AFTER_BUILD=0
      ;;
    *)
      echo "Unknown option: $arg" >&2
      echo "Usage: ./scripts/install-dev-app.sh [--no-open] [--target-only]" >&2
      exit 1
      ;;
  esac
done

trap cleanup_install_artifacts EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ "$INSTALL_AFTER_BUILD" == "1" ]]; then
  for required in /usr/sbin/lsof osascript plutil open; do
    if ! command -v "$required" >/dev/null 2>&1; then
      echo "Required macOS install tool is unavailable: $required" >&2
      exit 1
    fi
  done
  acquire_install_lock
fi

if [[ -n "$SIGNING_IDENTITY" ]]; then
  if ! security find-identity -v -p codesigning | grep -Fq "$SIGNING_IDENTITY"; then
    echo "Signing identity not found: $SIGNING_IDENTITY" >&2
    echo "Set MINUTES_DEV_SIGNING_IDENTITY (preferred) or APPLE_SIGNING_IDENTITY to a valid codesigning identity in your keychain." >&2
    exit 1
  fi
  SIGN_MODE="identity"
fi

echo "=== Preparing canonical build helpers ==="
# build.rs writes two tracked helper paths. Preserve the developer's local
# copies, build from HEAD-owned source state, and restore those copies only
# after the signed bundle has been completely produced.
prepare_canonical_build_helpers

echo "=== Building CLI (release) ==="
assert_clean_build_source
echo "Build commit: $MINUTES_BUILD_COMMIT"
run_with_ort_retry cargo build --release -p minutes-cli --features "$MINUTES_BUILD_FEATURES"

echo "=== Staging CLI as Tauri sidecar ==="
HOST_TARGET="$(rustc -Vv | awk '/host:/ {print $2}')"
mkdir -p tauri/src-tauri/bin
cp -f target/release/minutes "tauri/src-tauri/bin/minutes-${HOST_TARGET}"

echo "=== Building ${DEV_PRODUCT_NAME}.app ==="
# build.rs compiles all three Swift helpers into the external-bin staging
# directory, and Tauri bundles their fresh target-specific copies.
echo "=== Forcing fresh native helper generation ==="
cargo clean -p minutes-app
remove_generated_build_helpers
run_with_ort_retry cargo tauri build --bundles app --config "$DEV_CONFIG" --features "$MINUTES_BUILD_FEATURES" --no-sign
reset_tracked_build_helpers_to_head
assert_clean_build_source
# Inside-out signing (#311): sign every nested executable FIRST (the CLI
# sidecar with its own entitlements), then the outer bundle WITHOUT --deep.
# --deep re-signs nested code with the outer entitlements (clobbering the
# sidecar's), and any post-seal patching of nested code invalidates the
# bundle seal, so copied/downloaded apps fail Gatekeeper as "damaged".
SIDECAR_BIN="$BUILD_APP/Contents/MacOS/minutes"
if [[ "$SIGN_MODE" == "identity" ]]; then
  echo "=== Signing nested executables (inside-out) with configured identity ==="
  while IFS= read -r nested_executable; do
    if [[ "$nested_executable" == "$SIDECAR_BIN" ]]; then
      codesign --force --options runtime --timestamp \
        --entitlements tauri/src-tauri/minutes-cli.entitlements \
        --sign "$SIGNING_IDENTITY" \
        "$nested_executable"
    else
      codesign --force --options runtime --timestamp \
        --sign "$SIGNING_IDENTITY" \
        "$nested_executable"
    fi
  done < <(find "$BUILD_APP/Contents/MacOS" -maxdepth 1 -type f \( -perm -100 -o -perm -010 -o -perm -001 \))

  echo "=== Signing ${DEV_PRODUCT_NAME}.app (outer, no --deep) ==="
  codesign --force --options runtime --timestamp \
    --entitlements tauri/src-tauri/entitlements.plist \
    --sign "$SIGNING_IDENTITY" \
    "$BUILD_APP"
else
  echo "=== Signing ${DEV_PRODUCT_NAME}.app ad-hoc (inside-out) ==="
  echo "No MINUTES_DEV_SIGNING_IDENTITY / APPLE_SIGNING_IDENTITY configured."
  echo "Using ad-hoc signing so the app remains runnable for contributors."
  echo "TCC-sensitive features may still require re-granting permissions after rebuilds."
  while IFS= read -r nested_executable; do
    if [[ "$nested_executable" == "$SIDECAR_BIN" ]]; then
      codesign --force --options runtime \
        --entitlements tauri/src-tauri/minutes-cli.entitlements \
        --sign - "$nested_executable"
    else
      codesign --force --sign - "$nested_executable"
    fi
  done < <(find "$BUILD_APP/Contents/MacOS" -maxdepth 1 -type f \( -perm -100 -o -perm -010 -o -perm -001 \))
  codesign --force --sign - "$BUILD_APP"
fi

echo "=== Verifying bundle seal (strict) ==="
codesign --verify --deep --strict "$BUILD_APP" && echo "  Seal OK"

if [[ "$INSTALL_AFTER_BUILD" == "1" ]]; then
  echo "=== Installing ${DEV_PRODUCT_NAME}.app to ${INSTALL_DIR} ==="
  rm -rf "$STAGED_APP" "$BACKUP_APP"
  cp -rf "$BUILD_APP" "$STAGED_APP"
  echo "=== Verifying staged installed bytes ==="
  codesign --verify --deep --strict "$STAGED_APP" && echo "  Staged seal OK"
  if ! stop_running_dev_app; then
    exit 1
  fi
  if [[ -d "$INSTALL_APP" ]]; then
    /bin/mv -f "$INSTALL_APP" "$BACKUP_APP"
    INSTALL_SWAP_ACTIVE=1
  fi
  if ! /bin/mv -f "$STAGED_APP" "$INSTALL_APP"; then
    if [[ -d "$BACKUP_APP" ]]; then
      /bin/mv -f "$BACKUP_APP" "$INSTALL_APP"
      INSTALL_SWAP_ACTIVE=0
    fi
    echo "Could not atomically install ${DEV_PRODUCT_NAME}.app; the previous app was restored." >&2
    exit 1
  fi
  if ! codesign --verify --deep --strict "$INSTALL_APP"; then
    echo "Installed bundle seal verification failed; restoring the previous app." >&2
    restore_previous_app 0 || true
    exit 1
  fi
  echo "  Installed seal OK"

  echo "=== Running native hotkey diagnostic from installed dev app ==="
  set +e
  ./scripts/diagnose-desktop-hotkey.sh "$INSTALL_APP"
  DIAG_EXIT=$?
  set -e
else
  DIAG_EXIT="skipped"
fi

echo ""
if [[ "$INSTALL_AFTER_BUILD" == "1" ]]; then
  echo "Installed app: $INSTALL_APP"
else
  echo "Built app: $BUILD_APP"
fi
echo "Bundle id: com.useminutes.desktop.dev"
echo "Build features: $MINUTES_BUILD_FEATURES"
echo "Signing mode: $SIGN_MODE"
echo "Hotkey diagnostic exit code: $DIAG_EXIT"
echo "  0 = CGEventTap started successfully"
echo "  2 = Input Monitoring / macOS identity is still blocking the hotkey"
if [[ "$INSTALL_AFTER_BUILD" == "1" ]]; then
  echo ""
  echo "For TCC-sensitive testing, launch only this installed dev app."
  echo "Avoid the repo symlink (./Minutes.app), raw target bundles, or ad-hoc builds."
fi
if [[ "$SIGN_MODE" == "adhoc" ]]; then
  echo ""
  echo "Tip: export MINUTES_DEV_SIGNING_IDENTITY to a consistent local signing identity"
  echo "if you want more stable macOS permission behavior across rebuilds."
fi

if [[ "$INSTALL_AFTER_BUILD" == "0" ]]; then
  echo ""
  echo "Target-only mode: launch $BUILD_APP directly for packaging checks."
fi

if [[ "$OPEN_AFTER_INSTALL" == "1" && "$INSTALL_AFTER_BUILD" == "1" ]]; then
  echo ""
  echo "=== Launching ${DEV_PRODUCT_NAME}.app ==="
  LAUNCH_STARTED_UNIX_MS="$(( $(date +%s) * 1000 ))"
  if ! open -n "$INSTALL_APP"; then
    echo "macOS refused to launch the newly installed app; restoring the previous app." >&2
    restore_previous_app 1 || true
    exit 1
  fi
  if ! verify_frontend_startup "$LAUNCH_STARTED_UNIX_MS"; then
    echo "=== Restoring previous ${DEV_PRODUCT_NAME}.app after failed startup ===" >&2
    restore_previous_app 1 1 || true
    exit 1
  fi
  INSTALL_SWAP_ACTIVE=0
  rm -rf "$BACKUP_APP"
elif [[ "$INSTALL_AFTER_BUILD" == "1" ]]; then
  # --no-open deliberately skips the runtime readiness gate, but the staged
  # and installed bundle seals above still protect the replacement itself.
  INSTALL_SWAP_ACTIVE=0
  rm -rf "$BACKUP_APP"
fi
