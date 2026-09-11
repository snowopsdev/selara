# Pinned Selara Codex runtime

Selara bundles a writing-only binary built inside the official Codex 0.153.4
workspace. `runtime.toml` pins the peeled upstream commit, archive SHA-256,
ordered patch-set SHA-256, Rust 1.95.0, and macOS 11 ARM64 target. The patch
adds a small `selara-writing` binary crate and maintains authentication fixes;
it preserves the upstream package version. The release archive's workspace
lockfile still contains 0.0.0 entries, so the patch also normalizes those local
workspace versions to 0.153.4 without updating third-party dependencies.

Build with Python 3.12+ and the pinned Rust toolchain:

```sh
scripts/codex-runtime/build.sh
```

The builder verifies and caches the official archive, applies only the pinned
patch, and runs `cargo build --locked --release`. `SOURCE_ARCHIVE` may select an
offline copy of the same verified archive. It emits `target/selara-codex`,
`target/selara-codex.LICENSE`, `target/selara-codex.NOTICE`, and a provenance JSON
containing the build inputs and binary SHA-256. Source and compiler caches are
retained between builds; arbitrary local source trees are not accepted.

## Runtime boundary

The binary accepts only `--version` or this launch command:

```text
selara-codex app-server --listen stdio:// --selara-writing-mode
```

The JSON-RPC initialization request and response both require
`selaraWritingMode: 1`. The runtime constructs no Codex agent session, global
instructions provider, thread store, hooks, plugins, skills, MCP clients,
environment roots, or tool registry. The only inference request contains the
caller's explicit instructions and text, `tools: []`, `tool_choice: "none"`,
`parallel_tool_calls: false`, and `store: false`. It uses the upstream model
catalog and Responses SSE client with the fixed OpenAI ChatGPT endpoint.
Model-provided instructions and tool capabilities never enter the request.

The supported methods are initialize, account/read, account/login/start,
account/login/cancel, account/logout, model/list, thread/start, turn/start,
turn/interrupt, and thread/unsubscribe. Unknown methods and additional execution
parameters are rejected. A thread must be ephemeral and allows one text turn.
Only completed assistant text is emitted, followed by provider-reported usage
when available and a successful turn status. Tool output, failed streams,
truncation, cancellation, and timeout cannot produce successful completion.

The upstream config loader is used only to resolve the credential-store choice,
keyring backend, forced login/workspace settings, managed authentication policy,
residency, and system-proxy preference. Project config and execution rules are
skipped. User execution settings are not interpreted by this runtime. Enterprise
cloud requirements remain enabled. Managed custom ChatGPT endpoints and managed
network constraints unsupported by the fixed transport fail closed.

## Authentication coordination

The upstream login library owns OAuth, storage, and token refresh. All Selara
processes using the same canonical CODEX_HOME coordinate through `.auth.lock`.
A wrapper covers complete file/keyring/auto-store transactions, including
migration and fallback. Refresh has a separate `.auth.refresh.lock` across the
network exchange, reloads after acquiring it, and uses account/refresh-token
comparison inside the persistence transaction. A late refresh cannot overwrite
a newer login or restore credentials after logout.

OAuth callbacks capture an authentication generation and check it while saving.
A newer login, current-login cancellation, logout, or other authentication write
invalidates a pending callback. Canceling an older login cannot invalidate a
newer login. Generation records and file credentials are written with atomic
replacement. Cancellation also interrupts the asynchronous exchange.

Logout clears the file, direct keyring, encrypted keyring, and ephemeral Codex
authentication backends for the selected canonical home under one lock. This
works without loading local or cloud execution configuration; a backend deletion
failure returns an error. Selara's separate BYOK provider credentials are outside
this operation.

Stock Codex processes do not acquire these Selara locks. Token snapshot checks
reduce stale-write risk, but cross-process coordination with an unmodified
external Codex process is not guaranteed. No profile, API key, or credential
material is copied into the Selara application or its configuration.

## Verification

```sh
scripts/codex-runtime/test-contract.sh
scripts/codex-runtime/test-runtime.sh
scripts/codex-runtime/test-auth-lock.sh
```

The production smoke test checks the real shipped entry point. The transport
suite builds the same runtime with an unshipped test-only feature that permits a
loopback endpoint, synthetic managed-policy files, and the upstream mock keyring
for logout transactions; the production build
rejects those switches. Fixtures include hostile AGENTS, skills, MCP, notify,
user/project configuration, fragmented UTF-8 SSE, truncated streams, tool
output, and cancellation. Assertions inspect the actual outgoing HTTP request.
The auth suite also checks mixed file/keyring stores and deletion errors. It
spawns separate processes against synthetic stores and delayed
mock refresh responses to exercise refresh/logout, refresh/login, simultaneous
refresh, and pending-callback/logout races. Tests do not use real credentials or
contact a model service.
