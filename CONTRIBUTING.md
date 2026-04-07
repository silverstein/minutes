# Contributing to Minutes

Thanks for your interest in contributing! Minutes is a solo project that welcomes contributors — whether it's a bug fix, a feature, a docs improvement, or just filing an issue.

## Quick Start

```bash
git clone https://github.com/silverstein/minutes.git
cd minutes

# Rust (core engine + CLI)
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"  # macOS 26+
cargo build
cargo test -p minutes-core --no-default-features   # Fast (no whisper model)

# TypeScript (MCP server + SDK)
cd crates/sdk && npm install && npm test            # 30 unit tests
cd ../mcp && npm install && npm run build           # MCP server
```

### Prerequisites

- **Rust** (latest stable) + **cmake** (`brew install cmake`)
- **Node.js** 18+ (for MCP server and SDK)
- **Python 3** (optional — only for diarization via pyannote)

## Architecture

```
crates/
├── core/       Rust library — all audio/transcription logic
├── cli/        CLI binary — thin wrapper around core
├── reader/     Lightweight Rust meeting parser (no audio deps)
├── sdk/        TypeScript SDK — minutes-sdk npm package
│               (query meetings from any agent framework)
├── mcp/        MCP server — imports from sdk, adds CLI integration
│   └── ui/     Interactive dashboard (vanilla TS → single-file HTML)
└── tauri/      Menu bar app (Tauri v2)
```

**Key design:** `minutes-core` is shared by CLI, MCP server, and Tauri app. The TypeScript SDK (`minutes-sdk`) provides the same read-only capabilities for the JS/TS ecosystem.

## Where to Contribute

**Good first issues** are tagged — check the [issue tracker](https://github.com/silverstein/minutes/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22).

Some areas where help is especially welcome:
- **Windows/Linux testing** — the core works cross-platform but edge cases remain
- **MCP tools** — new read-only tools in the SDK (`crates/sdk/src/reader.ts`)
- **CLI commands** — new query/reporting commands in `crates/cli/src/main.rs`
- **Docs** — README, inline code comments, examples

### Testing on Linux without a Linux machine

This repo ships a GitHub Codespaces config so anyone can spin up a Linux dev
environment in one click — no Linux box required:

```bash
gh codespace create --repo silverstein/minutes
gh codespace ssh
.devcontainer/test-linux.sh   # full sanity suite
```

See [`.devcontainer/README.md`](.devcontainer/README.md) for what works in a
Codespace (whisper, diarize, MCP, all the file-processing paths) and what
doesn't (live recording — no audio hardware in the container).

## Adding a Feature

### Rust (core/CLI)
1. Implement in `crates/core/src/`
2. Add error types to the module's error enum
3. Write unit tests in the same file
4. Wire into the CLI if user-facing
5. Run `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

### TypeScript (SDK/MCP)
1. Add functionality to `crates/sdk/src/reader.ts`
2. Add tests in `crates/sdk/src/reader.test.ts`
3. Run `cd crates/sdk && npm test`
4. If adding an MCP tool, wire it in `crates/mcp/src/index.ts`

## Running Tests

```bash
# Rust — fast (no whisper model needed)
cargo test -p minutes-core --no-default-features

# Rust — full (requires: minutes setup --model tiny)
cargo test

# TypeScript SDK
cd crates/sdk && npx vitest run

# MCP integration
cd crates/mcp && npm run build && node test/mcp_tools_test.mjs
```

124 tests total across Rust and TypeScript.

## Code Style

- `cargo fmt` for Rust formatting
- `cargo clippy -- -D warnings` must pass
- Per-module error enums (not `anyhow` in the library)
- File permissions `0600` on all meeting output
- Explicit > clever
- `String.includes()` not regex for user-input search (special char safety)

## License

MIT — see [LICENSE](LICENSE).
