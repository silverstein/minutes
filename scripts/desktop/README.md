# Recall desktop regression harness

The native chat controller lives in `tauri/src/scripts/recall-chat.js`. The shell injects its Tauri `invoke`/`listen` functions and existing safe Markdown renderer. The controller owns listener registration, turn rendering, errors, scrolling, send/stop controls, and reset. Terminal mode and window management remain in the shell. This is an incremental extraction; it does not replace the desktop UI.

## Run

After installing the SDK's existing TypeScript dependency:

```sh
node scripts/check_recall_chunk_contract.mjs
node crates/sdk/node_modules/typescript/bin/tsc -p tauri/tsconfig.recall.json
node --test scripts/check_recall_terminal_input_contract.test.mjs
```

Install Playwright 1.62.1 in an isolated repo runtime (or provide an existing runtime through `CODEX_PRIMARY_RUNTIME_NODE_MODULES`) and its browser, then run:

```sh
node -e "require('node:fs').mkdirSync('target/recall-browser', { recursive: true }); require('node:fs').writeFileSync('target/recall-browser/package.json', JSON.stringify({ private: true }))"
npm --prefix target/recall-browser install --no-save --package-lock=false --no-audit --no-fund playwright@1.62.1
node target/recall-browser/node_modules/playwright/cli.js install --with-deps chromium --only-shell
node scripts/desktop/recall-rendered.test.mjs
```

`MINUTES_DESKTOP_SCREENSHOT=/absolute/path/recall.png` optionally captures the final render. `PLAYWRIGHT_CHROMIUM_EXECUTABLE` and `PLAYWRIGHT_CHROMIUM_ARGS` (a JSON array) can select an already-installed browser in restricted test environments. Ordinary CI uses Playwright's pinned browser, without these overrides.

The explicit runtime directory prevents npm from resolving a parent package and changing shared user dependencies when this repository has no root package manifest.

The browser fixture imports the actual Recall panel DOM and CSS from `index.html`, the production controller, and the complete existing Markdown renderer block. Streaming commands/events are stubbed. Shell transition tests also stub window sizing and persistence. Additional geometry cases import the list and document panes and execute the production responsive policy, covering layout without the isolated panel override. Assertions cover provider dialects, partial/completed reply deduplication, tool/thinking status, malformed frames, unsafe Markdown, errors racing with render/done, scroll anchoring, Stop, reset, failed send, listener rollback/disposal, and narrow/wide composer geometry. Renderer extraction uses explicit source markers and fails if they disappear.

## Event contract

`tauri/src/contracts/recall-chat.json` generates both the runtime validator and TypeScript tagged union. After changing it, run:

```sh
node scripts/generate_recall_contract.mjs
```

The contract checker verifies freshness, exhaustive decoder cases, loaded shell scripts, and required keys in each explicit Rust JSON emission. The backend also forwards Claude JSON dynamically; static scanning cannot prove its shape. The runtime validator handles that boundary, ignores listed provider control frames, and reports unknown/malformed envelopes without logging their content. The controller further checks nested content before reading it. TypeScript checks the decoder's `never` exhaustiveness assertion and public controller interface.

## What this does not verify

This fixture proves browser rendering and interaction for native Chat, production layout geometry, and collapsed-panel focus behavior. It does not prove the complete app initialization, WebKit parity, actual provider IPC, recording, TCC permissions, global shortcuts, OS focus/insertion, or signed app installation. `docs/checklists/pre-commit.md` still requires the signed development app click-test on macOS before committing these UI changes. This work can be reviewed as an uncommitted patch while that gate is pending.

The existing event protocol has no frontend turn identifier: the controller can ignore stale command promise failures after a reset, but cannot distinguish a late event from an old backend turn after a new turn has started. Backend turn cancellation remains authoritative. Adding event IDs requires a coordinated backend protocol change, not an invented frontend guarantee.
