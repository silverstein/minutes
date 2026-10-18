# Release Checklist

**When shipping a new version, walk through every item in order.**

### 1. Version bump (every source must match)
```bash
# Preview the complete Domain-1 patch without touching this checkout, then apply it.
node scripts/bump-version.mjs --dry-run X.Y.Z
node scripts/bump-version.mjs X.Y.Z

# Keep the generated LLM documentation current, then run the public verifier.
node scripts/generate_llms_txt.mjs
node scripts/check_version_sync.mjs
```

The bump command updates every Domain-1 source, regenerates both npm lockfiles,
refreshes only the three Minutes workspace entries in `Cargo.lock`, regenerates
`site/lib/release.ts`, and verifies the result in a temporary git worktree before
applying one patch to the real checkout. It never changes the independently
versioned plugin metadata, `whisper-guard`, or the Tauri crate's own version.

For a plugin-only release, update the plugin trio with the same transactional
flow: `node scripts/bump-version.mjs --plugin X.Y.Z` (add `--dry-run` to preview).

**The bump changes a hash-sealed file.** `crates/cli/Cargo.toml` carries the
`minutes-core` version pin and is sealed as `cliCargo` in
`scripts/check_graph_worker_packaging.mjs`, so every bump fails that guard until
its golden hash is updated. That is the seal working, not a fault: confirm the
only change is the version pin, then reseal and re-run the guard.

```bash
git diff -- crates/cli/Cargo.toml          # expect only the minutes-core version
sha256sum crates/cli/Cargo.toml            # paste into cliCargo in check_graph_worker_packaging.mjs
node scripts/check_graph_worker_packaging.mjs
node scripts/check_graph_worker_packaging.mjs --self-test
```

CI enforces these checks. The pre-push hooks are optional local fast feedback — enable them with `scripts/setup-hooks.sh`. They can be bypassed with `git push --no-verify`, so a successful local push is never a substitute for green CI.

**Independent-cadence crates.** `crates/whisper-guard/Cargo.toml` is published to crates.io on its own cadence — it does NOT need to match the main version. Check whether it has unreleased changes before tagging the main release:
```bash
PUBLISHED=$(curl -s https://crates.io/api/v1/crates/whisper-guard | jq -r '.crate.max_stable_version')
LAST_PUBLISH_COMMIT=$(git log --grep="whisper-guard $PUBLISHED" --format="%H" | head -1)
git log "$LAST_PUBLISH_COMMIT"..HEAD -- crates/whisper-guard/   # any commits → bump + publish in Step 13
```

### 2. Manifest sync
- Tools in `manifest.json` match tools registered in `crates/mcp/src/index.ts`
- `long_description` reflects current capabilities
- `keywords` are current
- Regenerate the site constants and commit the result:
  ```bash
  node scripts/sync_site_release_version.mjs
  ```
  Per-PR CI tolerates a stale `MINUTES_TEST_COUNT` so a number that moves with every added test cannot redden unrelated checks (#666), but `release_readiness` runs `--check-release` at tag push and that tolerance does not apply. Do this before Step 10 or the tag fails. Step 15 refreshes the prose after publishing; this is the numbers, before tagging.

### 3. MCP runtime deps
All `import` statements in `crates/mcp/src/index.ts` must have their packages in `dependencies` (not `devDependencies`) in `package.json`. Smoke-test: `node -e "require('./crates/mcp/dist/index.js')"`

### 4. Build everything
```bash
cd crates/mcp && npm run build       # MCP server + dashboard UI
cargo fmt --all -- --check           # Rust formatting
cargo clippy --all --no-default-features -- -D warnings  # Rust lints
```

**macOS desktop note:**
- For local TCC-sensitive dogfooding before release, rebuild the dev app with:
```bash
export MINUTES_DEV_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
./scripts/install-dev-app.sh --no-open
```
- Do not treat a raw local `/Applications/Minutes.app` copy as the canonical test surface for permission-sensitive features.

### 5. Write release notes
Every release shows up in followers' GitHub feeds — this is free awareness. Write notes BEFORE creating the release. No release should ever ship with an empty body.
- Summarize what shipped and why it matters (not commit messages — outcomes)
- Include install instructions (cargo install, DMG, npx)
- Match the voice of past releases (see v0.8.0, v0.8.1 for examples)
- Save to the gitignored local file `notes-release-vX.Y.Z.md`

### 6. Push the release commit to `main` and wait for CI to go green
```bash
git push origin main
# The release preflight and pin step require this exact HEAD to be pushed.
gh run list --branch main --limit 3
gh run watch $(gh run list --branch main --limit 1 --json databaseId --jq '.[0].databaseId')
```
**Why this step exists**: registry publishing is authorized by the immutable
release tag, so the version bump and exact dependency pin must be reviewed and
green before that tag exists. The release script verifies the clean `main`
checkout, pushed HEAD, and version-sync policy.

### 7. Optional Phase 1 local pack-and-test preflight
```bash
node scripts/release.mjs phase1 X.Y.Z --dry-run
```

This credential-free preflight packs the SDK and tests MCP against that exact
tarball. It does not publish. The tag workflow repeats the package builds and
owns all registry mutations, so Phase 1 is useful before the irreversible tag
but is no longer required for authentication or publish ordering.

### 8. Phase 2: commit MCP's exact SDK pin
```bash
node scripts/release.mjs phase2 X.Y.Z
```

Phase 2 pins `crates/mcp/package.json` to the exact SDK version, regenerates the
MCP lockfile, and refuses to continue unless those are the only two changed
files. It creates the commit `release: pin minutes-sdk X.Y.Z for mcp` itself.
Push that commit and wait for CI on the new exact HEAD:

```bash
git push origin main
gh run list --branch main --limit 3
gh run watch $(gh run list --branch main --limit 1 --json databaseId --jq '.[0].databaseId')
```

Do not amend the Phase-2 commit or edit release inputs after this point. The
tag-triggered registry workflow checks out this exact commit and reruns
`check_version_sync.mjs --release` before either publish job can start.

### 9. Create the GitHub release as a DRAFT
```bash
gh release create vX.Y.Z -t "vX.Y.Z: Short Title" -F notes-release-vX.Y.Z.md --target "$(git rev-parse HEAD)" --draft
```

This stages the notes without announcing the release. Keep it as a draft while
the tag-triggered workflows build and attach artifacts. Creating the draft does
not create or push the local annotated tag used by the committed release flow.

### 10. Create and push the release tag
```bash
node scripts/release.mjs tag X.Y.Z
# Run the exact tag-push command printed by the script, for example:
git push origin vX.Y.Z
```

The tag command requires a clean, pushed HEAD and green CI, enforces the exact
pin, and creates an annotated local tag without pushing it. It has no registry
credentials and does not publish packages.

If `gh` is unavailable, Phase 3 refuses to proceed unless
`--skip-ci-check` is supplied explicitly. Use that escape hatch only after
manually confirming CI is green on `git rev-parse HEAD`.

Pushing the printed tag command fires the three artifact workflows and
`release-publish.yml`. The registry workflow publishes `minutes-sdk`, waits for
its exact version and integrity to be visible, then publishes `minutes-mcp`. In
parallel it publishes `minutes-core`, waits for its crates.io API visibility,
then publishes `minutes-cli`. Every publish is idempotent for safe workflow
reruns. See [Trusted publishing setup](trusted-publishing.md) for the one-time
registry configuration.

### 11. Wait for release assets and registry publishes, then publish the draft

```bash
gh run list --workflow="Release CLI Binaries" --limit 1
gh run list --workflow="Release macOS" --limit 1
gh run list --workflow="Release Windows Desktop" --limit 1
gh run list --workflow="Release Registry Packages" --limit 1

# After all four are green, registry versions are visible, and assets are attached:
gh release edit vX.Y.Z --draft=false
```

The artifact workflows attach the CLI binaries, DMG, Windows installers,
updater files (`latest.json`, `Minutes.app.tar.gz`), and `SHA256SUMS.txt`. The
registry workflow summary lists all four published or integrity-verified
versions. Publishing the draft is the announcement moment: it appears in
followers' feeds and becomes "latest". If any release workflow fails, do not
move or replace the tag; rerun an idempotent job where appropriate, or follow
the immutable-tag recovery policy in `channels.md` and cut a new patch release.

### 12. Confirm the .mcpb was built and attached

The `Desktop Extension bundle` job in `Release CLI Binaries` packs it, runs the
bundle guard, asserts the manifest version equals the tag, and attaches it to
the draft. Its checksum lands in `SHA256SUMS.txt` like every other asset.

**Confirm it, do not assume it.** This was a manual step until v0.25.1, where
every other asset was attached automatically and the `.mcpb` was missing, in a
release that existed specifically to fix the extension. Nothing failed; the step
was simply skipped, and it was noticed only by counting assets against the
previous release.

```bash
gh release view vX.Y.Z --json assets --jq '.assets[].name' | grep mcpb
grep mcpb SHA256SUMS.txt      # the bundle now has a published checksum
```

If you ever need to build it by hand, for a re-upload or a listing fix, note
that the Claude Desktop listing renders `manifest.mcpb.json`, not
`manifest.json`, and `pack_mcpb.sh` swaps the former into the bundle. Editing
only `manifest.json` leaves the listing text unchanged.

```bash
./scripts/pack_mcpb.sh minutes.mcpb   # not `mcpb pack .`
./scripts/check_mcpb_bundle.sh minutes.mcpb
gh release upload vX.Y.Z minutes.mcpb --clobber
```

There are no manual npm publish commands. `release-publish.yml` owns
SDK-before-MCP ordering, exact-integrity checks, OIDC provenance, and idempotent
retries.

### 13. Publish independent-cadence crates (whisper-guard) if bumped
Skip this step if Step 1 showed no changes to `crates/whisper-guard/` since the last whisper-guard publish.
```bash
cd crates/whisper-guard
cargo publish --dry-run                  # verify packaging cleanly
cargo publish                            # actual publish
# Confirm:
sleep 30 && curl -s https://crates.io/api/v1/crates/whisper-guard | jq '.crate.max_stable_version'
```
whisper-guard is a standalone MIT crate consumed outside this repo (currently 277+ downloads). Bump independently of the main release; do NOT couple to the Minutes version. If you skip the publish, the crates.io users miss the fix and you create silent drift between repo state and published artifact.

### 14. Verify minutes-core and minutes-cli on crates.io

As of #79 the workspace has no git dependencies (cpal is on crates.io 0.18.1 with `windows-core` pinned to 0.61.2; pyannote-rs is on crates.io 0.3.4), so these crates can be published again. They were last on crates.io at v0.9.4 and now publish at the main release version (currently 0.18.5).

The trusted-publishing workflow publishes in dependency order,
`minutes-core` before `minutes-cli`, because `minutes-cli` depends on the
crates.io version of `minutes-core`. whisper-guard (Step 13) must already be
published at the version `minutes-core` requires.

```bash
gh run view $(gh run list --workflow="Release Registry Packages" --limit 1 --json databaseId --jq '.[0].databaseId')
curl -sS -H 'User-Agent: minutes-release-verify (https://github.com/silverstein/minutes)' \
  https://crates.io/api/v1/crates/minutes-core/X.Y.Z | jq -r '.version.num'
curl -sS -H 'User-Agent: minutes-release-verify (https://github.com/silverstein/minutes)' \
  https://crates.io/api/v1/crates/minutes-cli/X.Y.Z | jq -r '.version.num'
```

Notes:
- `cargo publish` reads each crate's `version =` dependency fields (not the local `path =`), which already point at the crates.io versions, so no manifest edits are needed.
- Publishing is irreversible: you can only yank, never replace a version. Never move a failed release tag to replace published crate contents.
- This revives `cargo install minutes-cli` for users who do not use Homebrew.
- If the workflow's `minutes-core` publish fails with a missing-dependency error, confirm whisper-guard at the required version is already indexed (Step 13), then rerun `release-publish.yml` for the same tag.

### 15. Refresh the landing page copy, then redeploy
Before deploying, make sure the site matches what just shipped:

1. **Regenerate the stat line** (version, tool count, CLI count, test count):
   ```bash
   node scripts/sync_site_release_version.mjs
   ```
   The `Site Release Link Consistency` CI job runs this with `--check` on every push, which allows a stale test count through so unrelated PRs do not go red (#666). `release_readiness` runs `--check-release` at tag push and does not, so the numbers should already be current from Step 2; this step is for anything that changed since.
2. **Hand-update the prose** — the changelog strip and headline feature blurb in `site/app/page.tsx`, plus `docs/architecture/frontmatter-schema.md`'s "corresponds to" footer if the schema row moved. The sync script handles numbers; it cannot rewrite copy that references last release's headline features.
3. **Refresh social proof + comparison freshness** — update `site/lib/proof.ts` (stars/forks/contributors from the GitHub API, npm monthly downloads from `api.npmjs.org`) and spot-check the homepage comparison table cells plus `/compare/*` pages against competitors' current public docs. Competitor capabilities drift; stale cells cost more credibility than they buy.
4. **Build the exact static artifact**:
   ```bash
   npm --prefix site ci
   npm --prefix site run check:llms
   npm --prefix site run build
   ```

Commit and push the validated site changes to `main`. The Cloudflare Pages
project `useminutes` watches only `site/*`, builds from `site/`, and publishes
`site/out/`; changes elsewhere in the repository do not trigger a website
build.

For an operator-controlled recovery deploy, authenticate Wrangler with
`CLOUDFLARE_API_TOKEN`, then run from `site/`:

```bash
npx --yes wrangler@4.114.0 pages deploy out \
  --project-name useminutes \
  --branch main
```

Verify `https://useminutes.pages.dev`, `https://www.useminutes.app`, and
`https://useminutes.app` after deployment. `/llms.txt` must remain
`text/plain; charset=utf-8` with `Cache-Control: public, max-age=3600`.

**Assert the canonical host serves directly — do not eyeball this in a browser.**
A browser follows redirects silently, so a misrouted apex still renders the site
and passes a visual check while telling Google the canonical URL is a redirect.
Every canonical tag and every `sitemap.xml` entry uses the apex, so the apex must
return `200`, not `3xx`:

```bash
for path in / /llms.txt /resources/hipaa-compliant-ai-note-taker; do
  code=$(curl -s -o /dev/null -w '%{http_code}' "https://useminutes.app$path")
  [ "$code" = 200 ] || echo "FAIL apex $path -> $code (canonical host must serve 200)"
done
# www must 301 to the apex, not the reverse
curl -s -o /dev/null -w 'www -> %{http_code} %{redirect_url}\n' -I https://www.useminutes.app/
# the declared canonical must match the host that served the page
curl -s https://useminutes.app/ | grep -o '<link rel="canonical"[^>]*>'
```

This check exists because in 2026-08 the apex spent a month resolving to a stale
Vercel project that `307`-redirected to www. The site looked fine, and the entire
40-route content build stayed effectively unindexed: 496 referring domains, one
ranking keyword.

### 16. Homebrew tap: automated, but confirm it ran

`Update Homebrew Tap` (`.github/workflows/homebrew-tap.yml`) fires on
`release: published` and points both tap files at the new version. Publishing
rather than tagging is the trigger because that is the first moment the DMG the
cask hashes is guaranteed to exist.

**Confirm it, do not assume it.** The run appears under Actions; it prints the
before and after versions and then re-reads the tap over HTTPS the way `brew`
does, failing if either file disagrees with the release.

After writing, it re-runs its own bump in dry-run mode against the published
tap, so the postcondition covers the cask's `sha256` and not just the version
string: a cask carrying a correct version with a wrong hash fails every
install, and a version comparison would call it fine.

**The formula stages a second pinned asset.** Since silverstein/homebrew-tap#5
the formula compiles the CLI with `engine-sherpa` and installs the signed
plugin from that release's `minutes-macos-arm64-sherpa.tar.gz`, pinned by URL
and `sha256`. The bump script moves that pin with the tag and the verifier
checks it, so this needs no manual step. It is called out because the failure
is quiet: a formula whose version line is correct can still stage the previous
release's plugin, and the mismatch surfaces at transcription time as a loader
that refuses, not at install time.

If a release does not publish that archive, the workflow warns and leaves the
pin where it was rather than failing the release. Read that warning: it means
`brew install` is building a new CLI against an older plugin.

If `HOMEBREW_TAP_TOKEN` is missing the workflow warns and exits 0 rather than
turning a good release red, so a green release does not by itself prove the tap
moved. Check the run, or:

```bash
brew update && brew info silverstein/tap/minutes && brew info --cask silverstein/tap/minutes
```

The daily triage sweep also compares the latest release against both tap files,
so drift surfaces within a day even if nobody looks.

#### Doing it by hand (if the workflow is unavailable)
Two files in `silverstein/homebrew-tap`, and they drift independently. The
formula (`Formula/minutes.rb`, CLI) was faithfully bumped while the cask
(`Casks/minutes.rb`, desktop app) sat at 0.18.2 through six releases until a
user reported it (#736). The step that "only applies if the CLI changed" was
the one that kept happening; the unconditional one was the one forgotten.
Treat both as unconditional.

**Formula** — update the `tag:` to the new version:
```bash
SHA=$(gh api repos/silverstein/homebrew-tap/contents/Formula/minutes.rb --jq '.sha')
# Edit Formula/minutes.rb: change tag: "vX.Y.Z" → new version
# Push via API or clone+commit+push
```

**Cask** — update `version` and `sha256`. Compute the hash from the released
DMG itself, not from SHA256SUMS.txt, which does not list the DMG:
```bash
gh release download vX.Y.Z --pattern 'Minutes_X.Y.Z_aarch64.dmg'
sha256sum Minutes_X.Y.Z_aarch64.dmg
SHA=$(gh api repos/silverstein/homebrew-tap/contents/Casks/minutes.rb --jq '.sha')
# Edit Casks/minutes.rb: bump version + sha256, push via API
```

Verify both: `brew update && brew info silverstein/tap/minutes && brew info --cask silverstein/tap/minutes`.

Or run the same logic the workflow runs, which is safer than hand-editing
because it refuses rather than guesses when a tap file has been restructured:

```bash
HOMEBREW_TAP_TOKEN=... python3 scripts/bump_homebrew_tap.py --version X.Y.Z --dry-run
HOMEBREW_TAP_TOKEN=... python3 scripts/bump_homebrew_tap.py --version X.Y.Z
```
