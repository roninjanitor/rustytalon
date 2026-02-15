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

## Week 2: Smart Routing (TODO)

### Complexity Analyzer
- [ ] Create `src/llm/routing/mod.rs`
- [ ] Create `src/llm/routing/analyzer.rs` - query complexity scoring
- [ ] Create `src/llm/routing/strategy.rs` - routing strategies
- [ ] Implement complexity heuristics:
  - [ ] Token count estimation
  - [ ] Code detection
  - [ ] Multi-step reasoning detection
  - [ ] Domain classification

### Router Implementation
- [ ] Create `src/llm/routing/router.rs`
- [ ] Implement `SmartRouter` with strategy selection
- [ ] Add provider health tracking
- [ ] Add response quality validation

### Configuration
- [ ] Add routing config to `src/config.rs`
- [ ] Environment variables for routing preferences
- [ ] Cost thresholds and quality metrics

---

## Week 3: Configuration & Polish (TODO)

- [ ] Add fallback provider support
- [ ] Implement retry logic with exponential backoff
- [ ] Add cost tracking per request
- [ ] Create provider health dashboard data

---

## Week 4: Testing & Signal (TODO)

- [ ] Integration tests for each provider
- [ ] CLI testing
- [ ] Signal channel verification
- [ ] Docker deployment testing

---

## Week 5: Documentation & Launch (TODO)

- [ ] Update README.md
- [ ] Docker Compose examples
- [ ] API documentation
- [ ] Deployment guide
