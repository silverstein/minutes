# Windows Desktop Releases

This repository ships a dedicated GitHub Actions workflow at
[.github/workflows/release-windows-desktop.yml](/.github/workflows/release-windows-desktop.yml)
to build Windows desktop artifacts for the Tauri app.

The current goal is pragmatic: produce a runnable installer-first release path
for Windows users without blocking on code-signing infrastructure.

## What the workflow does

The `Release Windows Desktop` workflow:

- runs on `workflow_dispatch`
- also runs automatically for Git tags that start with `v`
- installs a pinned `tauri-cli` version from the workflow env so Windows
  release builds stay reproducible across reruns
- builds an unsigned NSIS installer with `cargo tauri build --bundles nsis --no-sign`
- keeps the raw desktop `.exe` as a fallback artifact
- uploads both artifacts to GitHub Actions
- attaches both artifacts to the GitHub Release when the workflow was triggered by a tag

## Artifacts

The workflow publishes two Windows desktop artifacts:

- `minutes-desktop-windows-x64-setup.exe`
  Preferred installer artifact built through Tauri's NSIS bundler.
- `minutes-desktop-windows-x64.exe`
  Raw desktop runner binary kept as a fallback for troubleshooting and advanced
  users.

## Signing status

The workflow intentionally uses `--no-sign` today.

That means:

- Windows users may see SmartScreen / unknown publisher warnings
- the installer is best treated as preview-quality distribution until signing is
  added
- release notes should explicitly say the Windows installer is currently unsigned

Signing is tracked separately so we can add certificate handling deliberately
instead of smuggling it into the first installer pipeline.

## Installer choice

Minutes currently prefers **NSIS** over MSI for the first Windows installer
pass because:

- it is directly supported by Tauri's built-in Windows bundling path
- it produces a single `.exe` installer that is easy to attach to GitHub Releases
- it avoids the extra product-version / upgrade-code overhead of WiX while the
  Windows desktop surface is still settling

MSI / WiX can still be evaluated later if enterprise deployment requirements
become important.

## Triggering a release

### Manual

Run the workflow from the GitHub Actions UI:

1. Open `Actions`
2. Choose `Release Windows Desktop`
3. Click `Run workflow`

### Tagged release

Push a version tag:

```bash
git tag v0.1.1
git push origin v0.1.1
```

That will:

- run the Windows desktop workflow
- build the unsigned NSIS installer and raw `.exe`
- attach both artifacts to the GitHub Release for that tag

For preview builds, use a prerelease tag such as:

```bash
git tag v0.2.0-beta.1
git push origin v0.2.0-beta.1
```

Preview release notes should call out the unsigned Windows installer explicitly.

## Local maintainer verification

On a Windows machine, maintainers should verify at least:

1. installer launches successfully
2. app opens from the installed shortcut
3. first-run recording/transcription works
4. Recall/settings windows open
5. uninstall works cleanly

If you only need a quick smoke build from source on Windows:

```powershell
cargo install tauri-cli --version 2.10.1 --locked
cd tauri/src-tauri
cargo tauri build --ci --bundles nsis --no-sign
```

## Notes

- The Windows-specific installer defaults live in
  [tauri.windows.conf.json](/tauri/src-tauri/tauri.windows.conf.json).
- macOS app identity / TCC-sensitive development guidance lives in
  [docs/development/desktop-development.md](/docs/development/desktop-development.md).
- The macOS-only bundle settings remain in
  [tauri.macos.conf.json](/tauri/src-tauri/tauri.macos.conf.json)
  so the shared config stays cross-platform.
- Channel and rollback rules live in
  [docs/release/channels.md](/docs/release/channels.md).
