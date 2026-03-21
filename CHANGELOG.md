# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Note:** RustyTalon v0.1.0+ is derived from [IronClaw](https://github.com/nearai/ironclaw) (Copyright 2024 NEAR AI).
> Earlier entries in this changelog reflect the history of that project and our enhancements.

## [Unreleased]

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
