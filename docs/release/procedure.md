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
git log "$LAST_PUBLISH_COMMIT"..HEAD -- crates/whisper-guard/   # any commits → bump + publish in Step 11.5
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
- Save to a temp file: `notes.md`

### 6. Push commits to `main` and wait for CI to go green
```bash
git push origin main
# Watch CI — do NOT tag or publish until the CI workflow succeeds on this commit.
gh run list --branch main --limit 3
gh run watch $(gh run list --branch main --limit 1 --json databaseId --jq '.[0].databaseId')
```
**Why this step exists**: if you tag first and CI breaks (e.g. a Windows cross-platform build failure), you've already published the version to the public GitHub feed, npm, and brew. Moving a tag after the fact is messy — force-pushing the tag drops the GitHub release to draft, requires re-publishing, and leaves the npm package / brew formula pointing at a different commit than the release tag. Catch the failure *before* any of that happens.

### 7. Create the GitHub release as a DRAFT
```bash
gh release create vX.Y.Z -t "vX.Y.Z: Short Title" -F notes.md --target main --draft
```
This stages the release with its notes, but does **not** create the git tag yet: GitHub creates the tag only when a draft is published. The three release workflows (`Release CLI Binaries`, `Release macOS`, `Release Windows Desktop`) trigger on `push: tags`, so they do NOT run while the release is a draft. The draft also does not announce in subscribers' feeds or mark "latest".

Do NOT `git tag` locally. The safety here is step 6: it already confirmed `main` CI is green on the exact commit `--target` points to, so publishing cannot announce a commit that failed CI.

### 8. Publish the release (creates the tag and triggers the binary workflows)
```bash
gh release edit vX.Y.Z --draft=false
```
Publishing creates the `vX.Y.Z` tag at the target commit, which fires the three release workflows. `Release macOS` verifies the release exists before uploading, so the release must be published first (you cannot build the binaries against a draft with these triggers). This is also the moment the version shows up in followers' feeds and becomes "latest".

### 9. Wait for the release workflows to upload assets
```bash
gh run list --workflow="Release CLI Binaries" --limit 1
gh run list --workflow="Release macOS" --limit 1
gh run list --workflow="Release Windows Desktop" --limit 1
```
They attach the CLI binaries, DMG, Windows installers, updater files (`latest.json`, `Minutes.app.tar.gz`), and `SHA256SUMS.txt`. If one fails, fix on `main`, delete the tag and release (`gh release delete vX.Y.Z --cleanup-tag --yes`), and re-run from step 7 at the new commit. Because main CI was green before the tag, failures here are usually packaging or signing, not code.

### 10. Build and upload .mcpb
```bash
./scripts/pack_mcpb.sh   # use this, not `mcpb pack .`; it swaps manifest.mcpb.json's Claude listing into the bundle
./scripts/check_mcpb_bundle.sh minutes.mcpb   # same guard CI runs; catches manifest drift before upload
gh release upload vX.Y.Z minutes.mcpb --clobber
```

### 11. Publish npm packages
```bash
cd crates/sdk && npm publish --access public --registry https://registry.npmjs.org
cd crates/mcp && npm publish --access public --registry https://registry.npmjs.org
```
**IMPORTANT**: `crates/mcp/package.json` must depend on `"minutes-sdk": "^X.Y.Z"` (npm version), NOT `"file:../sdk"` (local path). Check before publishing. If 2FA blocks publish, use a granular access token with "Bypass 2FA" enabled.

### 11.5. Publish independent-cadence crates (whisper-guard) if bumped
Skip this step if Step 1 showed no changes to `crates/whisper-guard/` since the last whisper-guard publish.
```bash
cd crates/whisper-guard
cargo publish --dry-run                  # verify packaging cleanly
cargo publish                            # actual publish
# Confirm:
sleep 30 && curl -s https://crates.io/api/v1/crates/whisper-guard | jq '.crate.max_stable_version'
```
whisper-guard is a standalone MIT crate consumed outside this repo (currently 277+ downloads). Bump independently of the main release; do NOT couple to the Minutes version. If you skip the publish, the crates.io users miss the fix and you create silent drift between repo state and published artifact.

### 11.6. Publish minutes-core and minutes-cli to crates.io

As of #79 the workspace has no git dependencies (cpal is on crates.io 0.18.1 with `windows-core` pinned to 0.61.2; pyannote-rs is on crates.io 0.3.4), so these crates can be published again. They were last on crates.io at v0.9.4 and now publish at the main release version (currently 0.18.5).

Publish in dependency order, `minutes-core` before `minutes-cli`, because `minutes-cli` depends on the crates.io version of `minutes-core`. whisper-guard (Step 11.5) must already be published at the version `minutes-core` requires.

```bash
# core first; cli depends on it. dry-run each before the real publish.
cd crates/core
cargo publish --dry-run
cargo publish
# wait for the index to pick up core so cli can resolve it
sleep 45 && curl -s https://crates.io/api/v1/crates/minutes-core | jq '.crate.max_stable_version'

cd ../cli
cargo publish --dry-run
cargo publish
sleep 30 && curl -s https://crates.io/api/v1/crates/minutes-cli | jq '.crate.max_stable_version'
```

Notes:
- `cargo publish` reads each crate's `version =` dependency fields (not the local `path =`), which already point at the crates.io versions, so no manifest edits are needed.
- Publishing is irreversible: you can only yank, never replace a version. Always run the `--dry-run` first.
- This revives `cargo install minutes-cli` for users who do not use Homebrew.
- If `minutes-core` fails to publish with a missing-dependency error, confirm whisper-guard at the required version is already indexed (Step 11.5).

### 12. Refresh the landing page copy, then redeploy
Before deploying, make sure the site matches what just shipped:

1. **Regenerate the stat line** (version, tool count, CLI count, test count):
   ```bash
   node scripts/sync_site_release_version.mjs
   ```
   The `Site Release Link Consistency` CI job runs this with `--check` on every push, so forgetting this step also blocks CI — but running it locally first saves a round-trip and surfaces drift before tagging.
2. **Hand-update the prose** — the changelog strip and headline feature blurb in `site/app/page.tsx`, plus `docs/architecture/frontmatter-schema.md`'s "corresponds to" footer if the schema row moved. The sync script handles numbers; it cannot rewrite copy that references last release's headline features.
3. **Refresh social proof + comparison freshness** — update `site/lib/proof.ts` (stars/forks/contributors from the GitHub API, npm monthly downloads from `api.npmjs.org`) and spot-check the homepage comparison table cells plus `/compare/*` pages against competitors' current public docs. Competitor capabilities drift; stale cells cost more credibility than they buy.
4. **Then deploy**:
   ```bash
   npx vercel@50.38.2 build --prod
   npx vercel@50.38.2 deploy --prebuilt --yes --prod --scope evil-genius-laboratory
   ```

**IMPORTANT**: Run these commands from the repo root, not `site/`. The linked Vercel project uses `rootDirectory=site`, and the Git-connected / remote build path is currently failing after successful Next 16.2.3 builds because Vercel looks for `.next/routes-manifest-deterministic.json`. The prebuilt flow uploads the local `.vercel/output` and avoids that failing server-side post-build step.

**Check before deploying**: `cat .vercel/project.json` should show `"projectName": "useminutes.app"` with `"framework": "nextjs"`. If it's pointing at a different project (e.g. `rx-vip/minutes`), the build produces an empty static tree (no `index.html`, no SSR functions) and the deploy aliases return 404. Fix the link before building.

### 13. Update Homebrew tap formula if CLI changed
The formula lives at `silverstein/homebrew-tap` → `Formula/minutes.rb`. Update the `tag:` to the new version:
```bash
# Fetch current SHA, update via GitHub API
SHA=$(gh api repos/silverstein/homebrew-tap/contents/Formula/minutes.rb --jq '.sha')
# Edit Formula/minutes.rb: change tag: "vX.Y.Z" → new version
# Push via API or clone+commit+push
```
Verify: `brew update && brew info silverstein/tap/minutes` should show the new version.
