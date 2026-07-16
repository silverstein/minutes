# Trusted publishing setup

The `release-publish.yml` workflow publishes `minutes-sdk`, `minutes-mcp`,
`minutes-core`, and `minutes-cli` from a protected `v*` tag. npm and crates.io
authenticate the workflow through GitHub's OpenID Connect (OIDC) identity, so
the repository must not contain registry tokens or registry publishing secrets.

Complete this one-time registry setup after `release-publish.yml` is present on
the repository's default branch. The owner, repository, and workflow filename
are security boundaries; enter them exactly as shown.

## npm

Configure each package separately:

1. Sign in to [npmjs.com](https://www.npmjs.com/) with a maintainer account.
2. Open `minutes-sdk` -> **Settings** -> **Trusted Publisher**.
3. Select **GitHub Actions** and enter:
   - Organization or user: `silverstein`
   - Repository: `minutes`
   - Workflow filename: `release-publish.yml`
   - Environment name: leave empty (the workflow does not use an environment)
   - Allowed action: enable `npm publish`
4. Save the trusted publisher.
5. Repeat the same click-path and values for `minutes-mcp`.

The workflow deliberately does not set `NODE_AUTH_TOKEN`. npm 11.5.1 or newer
detects the GitHub OIDC context automatically, and public trusted publishes
include provenance automatically.

After a tag run successfully publishes or verifies both packages through OIDC:

1. Revoke legacy automation and granular access tokens that were used for
   publishing these packages.
2. For each package, return to **Settings**, set publishing access to
   **Require two-factor authentication and disallow tokens**, and save it.

Do not tighten token access before the first OIDC run succeeds; keeping the
transition ordered preserves a recovery path if a registry-side field was
mistyped.

## crates.io

Configure each crate separately:

1. Sign in to [crates.io](https://crates.io/) with a crate owner account.
2. Open `minutes-core` -> **Settings** -> **Trusted Publishing**.
3. Add a GitHub trusted publisher with:
   - Organization or user: `silverstein`
   - Repository: `minutes`
   - Workflow filename: `release-publish.yml`
   - Environment name: leave empty
4. Save the trusted publisher.
5. Repeat the same click-path and values for `minutes-cli`.

The workflow uses the official `rust-lang/crates-io-auth-action@v1` action to
exchange the GitHub OIDC identity for a short-lived crates.io token. The action
revokes that temporary token when the job finishes. No `CARGO_REGISTRY_TOKEN`
secret should be added to GitHub.

`whisper-guard` is intentionally not configured for this workflow. It has an
independent version and publishing cadence documented in the main release
procedure.

## GitHub tag protection and rollout

The repository's existing ruleset protects `v*` tags. Keep that protection in
place: a pushed release tag authorizes all four irreversible registry
publishes, and neither trusted publisher configuration uses a GitHub
environment as an additional boundary.

Adding the workflow does not replay past tag events. In particular, the
already-pushed `v0.22.0` tag will not trigger `release-publish.yml`
retroactively. `workflow_dispatch` exists only for an intentional rerun of a
specific tag; the workflow checks out that tag, reruns release readiness, and
requires the tag name to match the committed package version.
