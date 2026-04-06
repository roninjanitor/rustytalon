# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Note:** RustyTalon v0.1.0+ is derived from [IronClaw](https://github.com/nearai/ironclaw) (Copyright 2024 NEAR AI).
> Earlier entries in this changelog reflect the history of that project and our enhancements.

## [Unreleased]

## [0.1.26] - 2026-04-06

### Added
- `owner_only` DM policy for Discord channel — only the configured `owner_id` can message the bot; all other senders are silently dropped (no pairing codes sent). This is now the default policy.
- WASM channel workspace state persistence — `workspace_write` calls from WASM callbacks are now flushed to an in-memory store shared across all callbacks; `workspace_read` reads from that store. Previously all WASM state (dm_policy, bot_id, dm_channel list, last_message_ids) was lost between each callback invocation.
- `DISCORD_DM_POLICY` environment variable — override the Discord `dm_policy` at runtime without editing capabilities files

### Changed
- Discord `dm_policy` default changed from `pairing` to `owner_only` — protects new deployments by default; set `DISCORD_DM_POLICY=pairing` or `DISCORD_DM_POLICY=open` to revert

### Fixed
- Discord bot no longer defaults to pairing-only flow when `DISCORD_DM_POLICY` env var is set but state was lost between WASM invocations

## [0.1.25] - 2026-04-06

### Fixed
- Discord Gateway handshake now logs credential count, credential keys, and whether unresolved placeholders remain — diagnoses token injection failures that produce 4004 Authentication failed

## [0.1.24] - 2026-04-06

### Added
- `docker-compose.dev.yml` — builds RustyTalon from local source instead of pulling from ghcr.io, enabling fast local iteration without waiting for CI image builds
- `dev/build-channels.sh` — builds all (or named) WASM channels and copies them into `dev/channels/` for use with the dev compose volume mount
- `dev/channels/` volume mount in dev compose — WASM channel changes can be tested by rebuilding the channel and restarting the container, no full image rebuild needed

## [0.1.23] - 2026-04-06

### Fixed
- Discord Gateway IDENTIFY now sent after receiving Hello (op 10) instead of immediately on connect — Discord requires this order and was closing the connection before any messages could be received
- Discord Gateway intents corrected to `37377` (GUILDS + GUILD_MESSAGES + DIRECT_MESSAGES + MESSAGE_CONTENT) — previous value `36864` was missing GUILDS and GUILD_MESSAGES, preventing the bot from receiving server channel messages
- WebSocket close frame code and reason now logged — previously swallowed, making Gateway rejections invisible in logs

### Added
- `wait_for_op` field in `HandshakeSchema` — allows WASM channel capabilities to declare a server op to wait for before sending the handshake payload (generalizes the Discord Hello → IDENTIFY pattern)
- Discord setup docs updated with required Discord privacy setting (Privacy & Safety → Allow direct messages from server members) and promoted to an explicit setup step

## [0.1.22] - 2026-04-05

### Added
- Connection broker for WASM channels — host-side persistent connection management (WebSocket, long-poll, SSE) that preserves sandbox security
- `on-event` WIT callback for WASM channels to receive events from persistent connections
- `connection` capability in channel `capabilities.json` schema for declaring persistent connection requirements
- Discord Gateway WebSocket configuration in `discord.capabilities.json` (enables real-time message delivery alongside existing polling)
- `on_event` handler in Discord WASM module for processing Gateway events

### Fixed
- WebSocket broker panic due to missing rustls `CryptoProvider` — installed `ring` provider at process startup so all TLS consumers (reqwest, tokio-tungstenite) work regardless of thread
- Docker onboarding wizard false-triggering when `DATABASE_BACKEND=libsql` is set but the DB file doesn't exist yet (libsql creates it on first use)

## [0.1.21] - 2026-04-01

### Fixed
- Extension settings (`extensions.discord.owner_id`, `extensions.discord.dm_policy`, etc.) saved via web UI now load into WASM channels on startup — previously silently ignored
- "Path not found" warnings on startup for `extensions.*` and `channel.enabled.*` DB settings that don't belong to the Settings struct
- `channels-src/` and `tools-src/` crates excluded from Cargo workspace so they build independently with `--target wasm32-wasip2`

## [0.1.20] - 2026-04-01

### Added
- Discord `owner_id` injection via `DISCORD_OWNER_ID` env var or `channels.discord_owner_id` setting — the bot now filters messages to the configured user, matching Telegram's existing owner binding

### Fixed
- Excessive `INFO`-level logging from WASM channel credential injection on every poll cycle (~30s) — downgraded 8 diagnostic log lines in `resolve_host_credentials` and `inject_host_credentials` to `DEBUG`

## [0.1.19] - 2026-03-31

### Fixed
- WASM channel wrapper missing host-based credential injection — channels like Discord that rely on the host to add `Authorization` headers (per capabilities.json) got 401 Unauthorized because only placeholder-based injection (`{TOKEN}`) was implemented for channels

## [0.1.18] - 2026-03-30

### Fixed
- Discord tool credential injection used `bearer` type (producing `Authorization: Bearer {token}`) instead of `header` type with `Bot` prefix — Discord API requires `Authorization: Bot {token}` format, causing 401 Unauthorized on all Discord tool API calls

## [0.1.17] - 2026-03-24

### Added
- `develop` branch as the integration branch for contributor PRs — all contributions should target `develop`, which is merged to `main` for releases
- `CONTRIBUTING.md` expanded with branching model, PR process, quality gate steps, and pointers to the adding-tools/channels/DB guides in CLAUDE.md
- Contributing section added to README with quick summary of the branch workflow

### Changed
- CI: tests now run on push to `develop` in addition to `main` and PRs; code style checks (fmt + clippy) also run on push to `develop`
- `main` branch is now protected — direct pushes blocked, PRs require 1 approving review and passing CI

### Fixed
- Removed remaining IronClaw branding from web UI (`index.html`, `app.js`, `style.css`), Windows installer (`wix/main.wxs`), deploy scripts, channel build scripts, and database migration comments
- Renamed `deploy/ironclaw.service` to `deploy/rustytalon.service`

## [0.1.16] - 2026-03-23

### Fixed
- Slack and WhatsApp setup guides returned 404 in the web UI — `SLACK_SETUP` and `WHATSAPP_SETUP` were missing from `ALLOWED_DOCS` in the docs endpoint, and the docs directory was not copied into the Docker image
- Telegram `dm_policy` enum in `telegram.capabilities.json` listed wrong values (`owner_only`/`anyone`); corrected to `allowlist`/`open` to match the actual channel code
- Rewrote `docs/TELEGRAM_SETUP.md` to be Docker-first and accurate (removed stale build-from-source instructions)

### Added
- `docs/SLACK_SETUP.md` — Docker-first Slack channel setup guide (bot token, OAuth scopes, socket mode, invite bot to channels)
- `docs/WHATSAPP_SETUP.md` — Docker-first WhatsApp channel setup guide (Meta developer app, phone number ID, webhook verification)

### Changed
- CI: Docker images are now only built and pushed on version tags (`v*`) — previously every push to `main` triggered a build, causing spurious image publishes between releases. PRs still get a test build without a push.

## [0.1.15] - 2026-03-23

### Fixed
- WASM channels with no credentials configured no longer start at all — previously, pre-installed channels (Telegram, Discord, Slack, WhatsApp, Matrix) would connect to their respective APIs on every boot without any token, risking IP reputation damage from unauthenticated requests. Channels are now skipped at startup unless at least one `{CHANNEL_NAME}_*` env var or stored secret is present.

### Changed
- Channels require credentials to activate — set `TELEGRAM_BOT_TOKEN`, `DISCORD_BOT_TOKEN`, etc. to enable the corresponding channel

## [0.1.14] - 2026-03-23

### Fixed
- Browser cached the old `app.js` after a container update, causing the channel enable/disable toggle to not appear — static asset handlers now send `Cache-Control: no-cache, must-revalidate`

## [0.1.13] - 2026-03-23

### Fixed
- Slack channel showed as "Not installed" in the Channels tab while simultaneously appearing as "Running" — the registry entry name was `"slack-channel"` but the WASM binary self-reports as `"slack"`, causing the UI merge to produce two separate cards instead of one

### Added
- Per-channel enable/disable toggle in the Channels tab — click **Disable** to persist the off state to the database; the channel is skipped on next restart. Click **Enable** to re-activate it. Changes take effect after a restart.
- `POST /api/channels/{name}/enable` and `POST /api/channels/{name}/disable` endpoints for programmatic channel management
- `enabled` field on `ChannelInfo` / `GET /api/channels` response — reflects the persisted enabled state from settings
- Regression test that verifies every `WasmChannel` registry entry name matches a known bundled channel name, preventing future name mismatches

### Changed
- **Channels tab now appears before Extensions** in the web UI navigation bar
- WASM channels with a disabled setting are skipped at startup rather than loaded and left idle

## [0.1.12] - 2026-03-23

### Fixed
- `GATEWAY_AUTH_TOKEN=changeme` default replaced with auto-generation: if the variable is unset, a random 32-character token is generated at startup and printed to the log as part of the web UI URL
- Dev `DATABASE_URL` default now matches `docker-compose.yml` credentials (`postgres://rustytalon:rustytalon@localhost/rustytalon`) — previous default caused auth failures when using the dev compose
- `SANDBOX_ENABLED=true` now documents the Docker socket volume mount requirement in `.env.example` and `docker-compose.prod.yml`
- `CLAUDE_CODE_ENABLED` now documents its `SANDBOX_ENABLED=true` prerequisite

### Added
- `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB`, and `DB_PORT` variables added to `.env.example` — these are required by `docker-compose.prod.yml` but were previously undocumented
- `docs/CONFIGURATION.md` is now the complete environment variable reference (all variables, defaults, and descriptions in one place); `README.md` links to it directly
- `docs/CONFIGURATION.md` includes a new PostgreSQL Compose Credentials section and updated Docker Sandbox notes

## [0.1.11] - 2026-03-23

### Fixed
- WASM channels (Discord, Telegram, Slack, Matrix, WhatsApp) were silently not installed in the Docker image — the `channels-builder` stage was missing `COPY wit/ /wit/`, so `cargo build` failed because each channel crate references `../../wit/channel.wit` which resolved to `/wit/channel.wit` inside the container
- Added missing `whatsapp/build.sh` so the WhatsApp channel is compiled and bundled alongside the other four channels

### Added
- WhatsApp channel is now pre-installed in the Docker image (was excluded from the build and install loops)

## [0.1.10] - 2026-03-20

### Fixed
- Missing `wasm_channels` field in `GatewayState` initializers in `ws.rs`, `ws_gateway_integration.rs`, and `openai_compat_integration.rs` — caused test compilation failures introduced by v0.1.9

## [0.1.9] - 2026-03-20

### Added
- Dedicated **Channels tab** in the web UI — WASM channels (Discord, Telegram, Slack, Matrix) now have their own panel showing running status and catalog entries, separate from the Extensions tab
- `GET /api/channels` endpoint — returns the list of loaded WASM channels with name, description, and running status
- `with_wasm_channels()` builder on `WebGateway` — wires startup-loaded channel names into the gateway state for the API
- `ChannelInfo` / `ChannelListResponse` types in `web/types.rs`
- **Bootstrap channel secrets from env** (`bootstrap_channel_secrets_from_env`) — on Docker deployments, environment variables like `DISCORD_BOT_TOKEN` are automatically stored encrypted in the DB on first run so channels activate without any CLI steps
- Fallback credential injection for deployments without `SECRETS_MASTER_KEY` — env vars with a channel-name prefix are injected directly so Docker works out of the box
- **Pre-built WASM channels in Docker image** — Discord, Telegram, Slack, and Matrix channels are compiled and bundled at image build time; users can configure them immediately via the web UI with no CLI required
- **Multi-arch Docker builds** — GitHub Actions now builds `linux/amd64` and `linux/arm64` images in parallel (native runners) and pushes a combined multi-arch manifest

### Changed
- WASM channels are filtered out of the Extensions tab — they appear in the new Channels tab instead
- `Channels` filter button removed from the Extensions kind-filter bar
- Docker default port updated from `3000` to `3001` in Dockerfile comments and `docker-compose.prod.yml`
- Dockerfile restructured into 5 stages (added `channels-builder` stage before the dependency planner)

## [0.1.8] - 2026-03-20

### Added
- Debug logging in `clean_response` when LLM output is fully stripped to empty string — shows raw content preview to aid diagnosis
- Warning log when an empty LLM response is silently discarded — previously this happened with no trace in logs

### Fixed
- `openai_compatible` provider now uses the Chat Completions API (`/v1/chat/completions`) instead of the OpenAI Responses API — prevents panic from rig-core when endpoints (e.g. Cloudflare Workers AI) don't implement the Responses API
- HTTP 400 errors from LLM providers now fail over immediately to the next provider instead of retrying 3x — 400s are client errors that will never succeed on retry; mapped to `ModelNotAvailable` so `FailoverProvider` handles them correctly while `TrackedProvider` skips the retry loop
- Upgraded rig-core 0.30 → 0.33

## [0.1.7] - 2026-03-20

### Added
- Inline config editor in the extensions tab — each installed extension now has a gear button (⚙) that opens a form panel rendered from `config_schema` in the extension's capabilities.json; values are saved to the settings table under `extensions.<name>.<field>` keys
- `config_schema` JSON Schema block in discord, telegram, and matrix channel capabilities files — describes non-secret configurable fields (owner_id, dm_policy, allow_from, homeserver, polling settings, etc.)
- `GET /api/extensions/{name}/config` — returns the config schema plus current saved values for a named extension
- `PUT /api/extensions/{name}/config` — saves config field values; validates field names against the schema (alphanumeric/underscore only) and rejects unknown fields to prevent key injection
- `installed: true` field on `InstalledExtension` responses — lets the web UI setup wizard distinguish installed extensions from catalog entries without a separate lookup
- `get_auth_info` now checks installed `McpServerConfig` first — remote servers without a pre-configured OAuth client correctly show a manual token entry form instead of a broken OAuth button

## [0.1.6] - 2026-03-20

### Added
- Discord WASM tool (`tools-src/discord/`) — bot-mode integration via Discord REST API v10; supports send message (with reply), list channels, get message history, add reaction, get user info, list/get guilds, and create threads; bot token auth with OAuth support
- Matrix WASM tool (`tools-src/matrix/`) — federated messaging via Matrix Client-Server API v3; supports any homeserver (matrix.org, Element, self-hosted); homeserver URL configurable via workspace at `matrix/homeserver`; actions: send message (plain text + HTML), list rooms, get messages, join/leave rooms, get profile, get room info, send read receipt, add reaction
- Unit test suites for both new tools: 13 tests for Discord (url encoding, action deserialization), 15 tests for Matrix (Matrix-specific sigil encoding for `!`, `@`, `$`, `:`, action deserialization with pagination and HTML formatting)

### Fixed
- `tools-src/*/Cargo.toml` files now include `[workspace]` table to opt out of the root Cargo workspace — fixes `cargo test` and `cargo fmt` being broken for all WASM tool crates (latent issue also affecting Slack, Telegram, and Google tool crates)

## [0.1.5] - 2026-03-20

### Added
- Extension catalog API (`GET /api/extensions/catalog`, `POST /api/extensions/catalog/search`) — browseable registry of all known extensions with category, auth type, install status, and build metadata
- Extension auth info endpoint (`GET /api/extensions/{name}/auth-info`) — returns structured auth instructions and OAuth availability for the setup wizard
- `ExtensionStatus` enum (`active` | `needs_auth` | `inactive` | `error`) — computed status now included in every `InstalledExtension` response
- `ExtensionAuthInfo` type — structured auth metadata (type, instructions, setup URL, token hint, OAuth flag) returned by the new auth-info endpoint
- Activation error tracking in `ExtensionManager` — last activation error per extension persisted in memory and surfaced in list responses
- `category` field on `RegistryEntry` — groups extensions by domain (e.g. `communication`, `productivity`, `infrastructure`) for catalog filtering
- 18 new built-in registry entries: Telegram, Slack, Discord, WhatsApp, Matrix channels; Gmail, Google Calendar, Docs, Drive, Sheets, Slides, Slack Tool, Telegram Tool, Okta WASM tools
- Docs endpoint (`GET /api/docs/{name}`) — serves allowlisted Markdown documentation files for in-app help rendering
- Web UI: extension catalog browser, per-extension auth setup wizard, docs viewer panel, extension status badges

### Changed
- `InstalledExtension` now includes `status` (`ExtensionStatus`) and `error` (last activation failure message, if any)

## [0.1.4] - 2026-03-19

### Added

- Discord DM channel (`channels-src/discord/`) — polls Discord DMs via REST every 30 s, DM pairing for access control, typing indicator, bot-token credential injection at the host boundary
- Multi-provider LLM failover and routing from .env configuration
  - All LLM backends with credentials present are now initialized (not just the primary)
  - Automatic failover between providers on transient errors when multiple backends are configured
  - Smart router can select from all available providers based on query complexity and cost
- GitHub Actions CI/CD workflow for multi-architecture Docker builds (amd64, arm64)
- Makefile with convenient development commands (docker-build, docker-up, docker-logs, ship quality gate, etc.)
- Deploy systemd service file for RustyTalon on GCP

### Changed

- Updated deploy scripts to use RustyTalon branding (replace IronClaw references)
- Enhanced README with three Docker quick-start options (pre-built images, build from source, local dev)
- Expanded Development section with Makefile command reference
- Improved build-all.sh script with helpful output and Makefile reference

### Documentation

- Added `docs/GETTING_STARTED.md` — first-run guide covering Docker, local binary, and PostgreSQL setup options
- Added `docs/WEB_UI.md` — full walkthrough of the browser UI (chat, slash commands, memory, jobs, routines, extensions, logs)
- Added `docs/MEMORY.md` — workspace/memory system guide covering well-known files, search, heartbeat, and usage tips
- Added `docs/ROUTINES.md` — creating and managing automated tasks with cron, event, webhook, and manual triggers
- Added `docs/TOOLS_AND_EXTENSIONS.md` — built-in tools reference, tool approval, installing WASM/MCP extensions, dynamic tool building
- Added `docs/CONFIGURATION.md` — complete environment variable reference organized by category
- Added `docs/DISCORD_SETUP.md` — Discord bot creation, DM pairing flow, and configuration reference
- Added NOTICE file with detailed attribution to IronClaw (nearai/ironclaw)
- Updated LICENSE-MIT to credit Nick Lozano and acknowledge IronClaw derivation
- Added Attribution section to README with link to original IronClaw project
- Updated README nav bar with links to all user guides
- Updated PRD to clarify derivation from IronClaw vs. standalone fork
- Fixed CHANGELOG to remove hardcoded nearai/rustytalon links (now relative for new repo)

## [0.1.3] - 2026-02-12

### Other

- Enabled builds caching during CI/CD
- Disabled npm publishing as the name is already taken

## [0.1.2] - 2026-02-12

### Other

- Added Installation instructions for the pre-built binaries
- Disabled Windows ARM64 builds as auto-updater [provided by cargo-dist] does not support this platform yet and it is not a common platform for us to support

## [0.1.1] - 2026-02-12

### Other

- Renamed the secrets in release-plz.yml to match the configuration
- Make sure that the binaries release CD it kicking in after release-plz

## [0.1.0] - 2026-02-12

### Added

- Add multi-provider LLM support via rig-core adapter
- Sandbox jobs
- Add Google Suite & Telegram WASM tools
- Improve CLI

### Fixed

- resolve runtime panic in Linux keychain integration

### Other

- Skip release-plz on forks
- Upgraded release-plz CD pipeline
- Added CI/CD and release pipelines
- DM pairing + Telegram channel improvements
- Fixes build, adds missing sse event and correct command
- Codex/feature parity pr hook
- Add WebSocket gateway and control plane
- select bundled Telegram channel and auto-install
- Adding skills for reusable work
- Fix MCP tool calls, approval loop, shutdown, and improve web UI
- Add auth mode, fix MCP token handling, and parallelize startup loading
- Merge remote-tracking branch 'origin/main' into ui
- Adding web UI
- Rename `setup` CLI command to `onboard` for compatibility
- Add in-chat extension discovery, auth, and activation system
- Add Telegram typing indicator via WIT on-status callback
- Add proactivity features: memory CLI, session pruning, self-repair notifications, slash commands, status diagnostics, context warnings
- Add hosted MCP server support with OAuth 2.1 and token refresh
- Add interactive setup wizard and persistent settings
- Rebrand to RustyTalon with security-first mission
- Fix build_software tool stuck in planning mode loop
- Enable sandbox by default
- Fix Telegram Markdown formatting and clarify tool/memory distinctions
- Simplify Telegram channel config with host-injected tunnel/webhook settings
- Apply Telegram channel learnings to WhatsApp implementation
- Merge remote-tracking branch 'origin/main'
- Docker file for sandbox
- Replace hardcoded intent patterns with job tools
- Fix router test to match intentional job creation patterns
- Add Docker execution sandbox for secure shell command isolation
- Move setup wizard credentials to database storage
- Add interactive setup wizard for first-run configuration
- Add Telegram Bot API channel as WASM module
- Add OpenClaw feature parity tracking matrix
- Add Chat Completions API support and expand REPL debugging
- Implementing channels to be handled in wasm
- Support non interactive mode and model selection
- Implement tool approval, fix tool definition refresh, and wire embeddings
- Tool use
- Wiring more
- Add heartbeat integration, planning phase, and auto-repair
- Login flow
- Extend support for session management
- Adding builder capability
- Load tools at launch
- Fix multiline message rendering in TUI
- Parse NEAR AI alternative response format with output field
- Handle NEAR AI plain text responses
- Disable mouse capture to allow text selection in TUI
- Add verbose logging to debug empty NEAR AI responses
- Improve NEAR AI response parsing for varying response formats
- Show status/thinking messages in chat window, debug empty responses
- Add timeout and logging to NEAR AI provider
- Add status updates to show agent thinking/processing state
- Add CLI subcommands for WASM tool management
- Fix TUI shutdown: send /shutdown message and handle in agent loop
- Remove SimpleCliChannel, add Ctrl+D twice quit, redirect logs to TUI
- Fix TuiChannel integration and enable in main.rs
- Integrate Codex patterns: task scheduler, TUI, sessions, compaction
- Adding LICENSE
- Add README with RustyTalon branding
- Add WASM sandbox secure API extension
- Wire database Store into agent loop
- Implementing WASM runtime
- Add workspace integration tests
- Compact memory_tree output format
- Replace memory_list with memory_tree tool
- Simplify workspace to path-based storage, remove legacy code
- Add NEAR AI chat-api as default LLM provider
- Add CLAUDE.md project documentation
- Add workspace and memory system (OpenClaw-inspired)
- Initial implementation of the agent framework
