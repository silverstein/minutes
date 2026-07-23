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

### 12. Build and upload .mcpb
```bash
./scripts/pack_mcpb.sh   # use this, not `mcpb pack .`; it swaps manifest.mcpb.json's Claude listing into the bundle
./scripts/check_mcpb_bundle.sh minutes.mcpb   # same guard CI runs; catches manifest drift before upload
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
   The `Site Release Link Consistency` CI job runs this with `--check` on every push, so forgetting this step also blocks CI — but running it locally first saves a round-trip and surfaces drift before tagging.
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

### 16. Update Homebrew tap formula if CLI changed
The formula lives at `silverstein/homebrew-tap` → `Formula/minutes.rb`. Update the `tag:` to the new version:
```bash
# Fetch current SHA, update via GitHub API
SHA=$(gh api repos/silverstein/homebrew-tap/contents/Formula/minutes.rb --jq '.sha')
# Edit Formula/minutes.rb: change tag: "vX.Y.Z" → new version
# Push via API or clone+commit+push
```
Verify: `brew update && brew info silverstein/tap/minutes` should show the new version.
