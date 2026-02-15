# RustyTalon Development Checklist

## Week 1: Fork & Remove NEAR AI Dependency

### Completed
- [x] Clone IronClaw repository
- [x] Initialize fresh git repo (remove IronClaw history)
- [x] Rebrand ironclaw → rustytalon (Cargo.toml, all source files)
- [x] Remove NEAR AI provider files
  - [x] Delete `src/llm/nearai.rs`
  - [x] Delete `src/llm/nearai_chat.rs`
  - [x] Delete `src/llm/session.rs` (NEAR AI OAuth)
- [x] Update `src/llm/mod.rs` - remove NEAR AI exports
- [x] Update `src/config.rs` - remove NearAiConfig, make Anthropic default
- [x] Update `src/workspace/embeddings.rs` - remove NearAiEmbeddings
- [x] Update `src/workspace/mod.rs` - remove NearAiEmbeddings export
- [x] Update `src/cli/status.rs` - show LLM provider status instead of session
- [x] Rewrite `src/setup/wizard.rs` - use env vars, no disk persistence
- [x] Update `src/main.rs` - remove all NEAR AI references
- [x] Verify compilation with `cargo check`

### Environment Variables (Docker-ready)
```bash
# Required
DATABASE_URL=postgres://user:pass@host:5432/rustytalon

# LLM Provider (pick one)
LLM_BACKEND=anthropic  # anthropic (default), openai, ollama, openai_compatible
ANTHROPIC_API_KEY=sk-ant-...
ANTHROPIC_MODEL=claude-sonnet-4-20250514

# Or for OpenAI
LLM_BACKEND=openai
OPENAI_API_KEY=sk-...
OPENAI_MODEL=gpt-4o

# Or for Ollama
LLM_BACKEND=ollama
OLLAMA_BASE_URL=http://localhost:11434
OLLAMA_MODEL=llama3

# Embeddings (optional, requires OpenAI)
OPENAI_API_KEY=sk-...
EMBEDDING_ENABLED=true
EMBEDDING_MODEL=text-embedding-3-small
```

---

## Week 2: Smart Routing

### Complexity Analyzer
- [x] Create `src/llm/routing/mod.rs`
- [x] Create `src/llm/routing/analyzer.rs` - query complexity scoring
- [x] Create `src/llm/routing/strategy.rs` - routing strategies
- [x] Implement complexity heuristics:
  - [x] Token count estimation
  - [x] Code detection
  - [x] Multi-step reasoning detection
  - [x] Domain classification

### Router Implementation
- [x] Create `src/llm/routing/router.rs`
- [x] Implement `SmartRouter` with strategy selection
- [x] Add provider health tracking
- [ ] Add response quality validation (deferred to Week 3)

### Configuration
- [x] Add routing config to `src/config.rs`
- [x] Environment variables for routing preferences
- [x] Cost thresholds and quality metrics

### Environment Variables for Routing
```bash
# Routing configuration
ROUTING_ENABLED=true                    # Enable smart routing (default: true)
ROUTING_STRATEGY=balanced               # balanced, cost, quality, local_first
ROUTING_MAX_COST=0.05                   # Max USD per request (optional)
ROUTING_MIN_QUALITY=0.5                 # Min quality score 0.0-1.0
ROUTING_ENABLE_FALLBACK=true            # Enable fallback providers
ROUTING_MAX_RETRIES=3                   # Max retry attempts
ROUTING_PREFERRED_PROVIDERS=anthropic,openai  # Comma-separated
ROUTING_EXCLUDED_PROVIDERS=             # Providers to exclude
```

---

## Week 3: Configuration & Polish

### Completed
- [x] Add fallback provider support (`SmartRouter::complete()` with health-aware fallback chain)
- [x] Implement retry logic with exponential backoff (`TrackedProvider` wraps any `LlmProvider`)
- [x] Add cost tracking per request (`TrackedProvider` records via `Database::record_llm_call()`)
- [x] Create provider health dashboard data (`ProviderHealthReport`, `GET /api/providers/health`)
- [x] Add response quality validation (`ResponseQualityChecker` with heuristic scoring)
- [x] Add LLM cost stats API endpoint (`GET /api/providers/costs`)
- [x] Add `get_llm_call_stats()` to Database trait (both postgres and libSQL backends)
- [x] Wire `TrackedProvider` into `main.rs` startup (wraps provider when DB available)
- [x] Wire `SmartRouter` into gateway via `with_smart_router()` and `with_llm_provider()`

---

## Week 4: Testing & Polish

### Test Infrastructure
- [x] Extract shared `MockProvider` to `src/llm/test_utils.rs` (single-call + multi-call variants)
- [x] Create `MockDatabase` in `src/db/test_utils.rs` (stub trait impl for unit tests)
- [x] Update `src/llm/failover.rs` tests to use shared `MockProvider`

### SmartRouter Tests (14 tests)
- [x] Route with single provider
- [x] Preferred provider selection
- [x] Excluded provider filtering
- [x] All providers excluded → error
- [x] Complete with fallback (primary fails, fallback succeeds)
- [x] Complete all providers fail → error
- [x] Health degrades after 3 failures
- [x] Health recovers after success
- [x] Health updated on success/failure after complete()
- [x] Fallback list excludes primary
- [x] No fallbacks when disabled

### TrackedProvider Tests (6 tests)
- [x] Delegates to inner provider
- [x] Records call in database on success
- [x] Model name delegates to inner
- [x] Retries transient errors and succeeds
- [x] No retry on auth errors
- [x] Exhausts retries and returns error

### CLI Parsing Tests (11 tests)
- [x] No args → default (run agent)
- [x] `run`, `status`, `tool list`, `tool install`, `config get`, `memory search`
- [x] `--cli-only`, `--no-db`, `-m "message"` flags
- [x] Invalid command → error

### Docker Deployment Verification (5 tests)
- [x] `Dockerfile` exists and contains cargo build
- [x] `Dockerfile.worker` exists and has entrypoint
- [x] `docker-compose.yml` exists and defines services

### Deferred
- [ ] Signal channel (not yet implemented — future feature, not a testing gap)

---

## Week 5: Documentation & Launch

### Completed
- [x] Update README.md (removed NEAR AI references, added multi-provider docs, quick start, API summary)
- [x] Update `.env.example` (organized by section, multi-provider config, all options documented)
- [x] Docker Compose production example (`docker-compose.prod.yml`)
- [x] API documentation (`docs/API.md` — all 56+ gateway endpoints with examples)
- [x] Deployment guide (`docs/DEPLOYMENT.md` — local, Docker, sandbox, multi-provider, security checklist)
