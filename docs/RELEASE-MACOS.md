# Signed macOS Releases

This repository ships a dedicated GitHub Actions workflow at
[.github/workflows/release-macos.yml](/.github/workflows/release-macos.yml)
to build a signed and notarized `Minutes.app` and `.dmg`.

The goal is simple: a non-technical macOS user should be able to install
Minutes without bypassing scary Gatekeeper warnings.

## What the workflow does

The `Release macOS` workflow:

- runs on `workflow_dispatch`
- also runs automatically for Git tags that start with `v`
- installs a pinned `tauri-cli` version from the workflow env so release builds
  stay reproducible across reruns
- builds the Tauri desktop bundle with macOS signing + notarization
- uploads the signed `Minutes.app`
- uploads the signed `.dmg`
- uploads the `.dmg` to an existing GitHub Release when the workflow was triggered by a tag

## Required GitHub secrets

Create these repository secrets before running the workflow:

| Secret | What it should contain |
|--------|-------------------------|
| `APPLE_CERTIFICATE` | Base64-encoded Developer ID Application certificate exported as `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Password used when exporting the `.p12` certificate |
| `APPLE_SIGNING_IDENTITY` | Developer ID Application identity name, for example `Developer ID Application: Example, Inc. (TEAMID)` |
| `APPLE_API_ISSUER` | App Store Connect issuer ID |
| `APPLE_API_KEY` | App Store Connect API key ID |
| `APPLE_API_PRIVATE_KEY` | Raw contents of the `AuthKey_<KEYID>.p8` file |

The workflow writes `APPLE_API_PRIVATE_KEY` into a temporary `.p8` file and
exports `APPLE_API_KEY_PATH` for the Tauri bundler.

## How to create the certificate secret

On a Mac that already has the Developer ID Application certificate installed:

```bash
security find-identity -v -p codesigning
```

Export the correct certificate as a `.p12`, then convert it to base64:

```bash
base64 -i Minutes-Developer-ID.p12 | pbcopy
```

Paste that value into the `APPLE_CERTIFICATE` GitHub secret.

## Triggering a release

### Manual

Run the workflow from the GitHub Actions UI:

1. Open `Actions`
2. Choose `Release macOS`
3. Click `Run workflow`

### Tagged release

Create the GitHub Release first, with notes, and let that create the tag:

```bash
scripts/release_notes.sh HEAD stable > notes.md
gh release create v0.1.1 -t "v0.1.1: Short title" -F notes.md --target main
```

That will:

- create the release with a non-empty body
- create the matching remote tag
- trigger the same macOS workflow
- build the signed + notarized artifacts
- upload the `.dmg` to the existing GitHub Release for that tag

For preview builds, use a prerelease tag such as:

```bash
scripts/release_notes.sh HEAD preview > notes.md
gh release create v0.2.0-beta.1 -t "v0.2.0-beta.1: Preview title" -F notes.md --target main --prerelease
```

The workflow will upload artifacts to that prerelease instead of creating one.

## Local maintainer verification

Before trusting the workflow end-to-end, maintainers should verify a signed
artifact locally after download:

```bash
spctl -a -vv target/release/bundle/macos/Minutes.app
codesign --verify --deep --strict --verbose=2 target/release/bundle/macos/Minutes.app
```

For a workflow-produced artifact, use the downloaded `.app` or mount the `.dmg`
first and then run the same commands.

## Notes

- The workflow is intentionally explicit rather than magical. If signing or
  notarization fails, the logs should point directly at the failing secret or
  Apple step.
- On tag builds, CI now refuses to create a release on your behalf. If
  `gh release view <tag>` fails, fix the release creation step first instead of
  pushing the tag directly.
- The workflow pins `TAURI_CLI_VERSION` in YAML. Bump that version
  intentionally when we decide to move the release toolchain forward.
- The workflow currently builds the runner-native macOS bundle. If universal
  binaries become a requirement later, track that separately instead of
  complicating this first trusted pipeline.
- Windows desktop release guidance lives in
  [docs/RELEASE-WINDOWS.md](/docs/RELEASE-WINDOWS.md).
- Channel and rollback rules live in
  [docs/RELEASE-CHANNELS.md](/docs/RELEASE-CHANNELS.md).
- Reproducible release-note generation lives in
  [docs/RELEASE-NOTES.md](/docs/RELEASE-NOTES.md).
