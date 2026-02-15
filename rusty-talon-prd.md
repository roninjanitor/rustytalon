# RustyTalon PRD (Product Requirements Document)

## Executive Summary

**Product Name:** RustyTalon  
**Domain:** rustytalon.com  
**Version:** 1.0.0  
**Target Launch:** 6 weeks from start  
**Primary Goal:** Create a secure, multi-provider AI assistant in Rust with smart routing capabilities and no vendor lock-in

## Problem Statement

Current AI assistant solutions suffer from:
- **Vendor Lock-in**: RustyTalon requires NEAR AI account, OpenClaw lacks type safety
- **Cost Inefficiency**: No smart routing between cheap and expensive models
- **Security Concerns**: Python/TypeScript implementations lack memory safety guarantees
- **Limited Control**: Can't mix providers or implement custom routing logic

## Solution

RustyTalon is a Rust-based fork of RustyTalon that:
1. Removes NEAR AI dependency
2. Supports direct API access to multiple providers (Anthropic, OpenAI, Liquid AI, etc.)
3. Implements intelligent routing between cheap and expensive models
4. Maintains RustyTalon's security architecture (WASM sandbox, credential protection)
5. Integrates with existing homelab infrastructure (PostgreSQL, Infisical, Signal)

---

## Technical Architecture

### Core Components

```
┌─────────────────────────────────────────────────────────────┐
│                        User Interfaces                       │
│         Signal • Telegram • HTTP API • CLI (REPL)           │
└─────────────────┬───────────────────────────────────────────┘
                  │
┌─────────────────▼───────────────────────────────────────────┐
│                     Smart Router                             │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐            │
│  │ Complexity │→ │  Routing   │→ │  Provider  │            │
│  │ Analyzer   │  │  Strategy  │  │  Selector  │            │
│  └────────────┘  └────────────┘  └────────────┘            │
└─────────────────┬───────────────────────────────────────────┘
                  │
        ┌─────────┴─────────┬──────────────┬────────────┐
        ▼                   ▼              ▼            ▼
┌───────────────┐  ┌──────────────┐  ┌─────────┐  ┌────────┐
│   Anthropic   │  │    OpenAI    │  │ Liquid  │  │  Groq  │
│    Provider   │  │   Provider   │  │   AI    │  │        │
└───────────────┘  └──────────────┘  └─────────┘  └────────┘
                          │
        ┌─────────────────┴─────────────────┐
        ▼                                   ▼
┌───────────────┐                  ┌─────────────────┐
│ PostgreSQL    │                  │  WASM Sandbox   │
│ + pgvector    │                  │  (Tools/Skills) │
└───────────────┘                  └─────────────────┘
```

### Technology Stack

**Language:** Rust 1.85+  
**Database:** PostgreSQL 15+ with pgvector extension  
**Secret Management:** Infisical (existing homelab setup)  
**Sandbox:** WebAssembly (WASM) for tool execution  
**API Clients:**
- `anthropic-sdk` v0.2
- `async-openai` v0.23
- `reqwest` v0.12 (for Liquid AI and other HTTP-based providers)

**Dependencies:**
```toml
[dependencies]
tokio = { version = "1.40", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
async-trait = "0.1"
anthropic-sdk = "0.2"
async-openai = "0.23"
reqwest = { version = "0.12", features = ["json"] }
sqlx = { version = "0.8", features = ["postgres", "runtime-tokio"] }
pgvector = "0.4"
wasmtime = "26"
infisical-rs = "0.3"
tracing = "0.1"
tracing-subscriber = "0.3"
```

---

## Feature Requirements

### P0 (Must Have for MVP)

#### FR-001: Multi-Provider Support
**Description:** Support multiple LLM providers with unified interface  
**Acceptance Criteria:**
- [ ] Anthropic provider implementation (Claude Opus 4.5, Sonnet 4.5, Haiku 4.5)
- [ ] OpenAI provider implementation (GPT-4o, GPT-4o-mini)
- [ ] Provider factory pattern for easy addition of new providers
- [ ] Common error handling across all providers
- [ ] Consistent message format translation

**Technical Details:**
- Implement `LlmProvider` trait in `src/llm/provider.rs`
- Create provider implementations in `src/llm/providers/`
- Support both streaming and non-streaming responses
- Handle tool/function calling for compatible providers

#### FR-002: Smart Routing System
**Description:** Intelligently route requests to appropriate provider based on complexity and strategy  
**Acceptance Criteria:**
- [ ] Complexity analyzer evaluates message difficulty (Simple/Medium/Complex)
- [ ] Three routing strategies: CostOptimized, QualityFirst, Balanced
- [ ] Automatic fallback if primary provider fails
- [ ] Quality checking with escalation for poor responses
- [ ] Cost tracking per request

**Routing Logic:**
```
CostOptimized:
  1. Send to primary (cheap) provider
  2. Evaluate response quality
  3. If quality insufficient, escalate to fallback
  4. Return best response

QualityFirst:
  1. Always send to fallback (best) provider
  2. Return response

Balanced:
  1. Analyze message complexity
  2. If Simple/Medium → CostOptimized flow
  3. If Complex → QualityFirst flow
```

**Complexity Signals:**
- Token count > 200 = Complex
- Contains code blocks = Complex
- Tool/function calling = Complex
- Keywords: "analyze", "explain", "debug" = Medium
- Default = Simple

#### FR-003: Configuration Management
**Description:** Flexible configuration system with secure secret management  
**Acceptance Criteria:**
- [ ] TOML-based settings file (`~/.rustytalon/settings.toml`)
- [ ] Infisical integration for API key retrieval
- [ ] Environment variable support
- [ ] Interactive onboarding wizard
- [ ] Settings validation on startup

**Configuration Schema:**
```toml
[database]
url = "postgresql://localhost/rustytalon"

[providers]
primary = "liquid/lfm-40b"
fallback = "anthropic/claude-sonnet-4-5"
routing_strategy = "Balanced"

[security]
enable_wasm_sandbox = true
allowed_endpoints = [
    "api.anthropic.com",
    "api.openai.com",
    "api.liquid.ai"
]

[channels]
signal_enabled = true
signal_device_name = "RustyTalon"
telegram_enabled = false
```

#### FR-004: PostgreSQL Memory System
**Description:** Persistent memory storage with vector search capabilities  
**Acceptance Criteria:**
- [ ] Store conversation history
- [ ] Vector embeddings for semantic search
- [ ] Efficient retrieval of relevant context
- [ ] Support for multiple conversations/users
- [ ] Automatic cleanup of old data

**Database Schema:**
```sql
CREATE TABLE conversations (
    id UUID PRIMARY KEY,
    user_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL,
    updated_at TIMESTAMP NOT NULL
);

CREATE TABLE messages (
    id UUID PRIMARY KEY,
    conversation_id UUID REFERENCES conversations(id),
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    provider_used TEXT,
    tokens_input INT,
    tokens_output INT,
    cost_usd DECIMAL(10, 8),
    created_at TIMESTAMP NOT NULL,
    embedding vector(1536)
);

CREATE INDEX ON messages USING ivfflat (embedding vector_cosine_ops);
```

#### FR-005: WASM Tool Sandbox
**Description:** Secure execution environment for tools/skills  
**Acceptance Criteria:**
- [ ] Preserve RustyTalon's WASM sandbox implementation
- [ ] Capability-based permissions (HTTP, filesystem, secrets)
- [ ] Endpoint allowlisting for HTTP requests
- [ ] Credential injection at host boundary (never exposed to WASM)
- [ ] Resource limits (memory, CPU, execution time)

**Security Requirements:**
- All untrusted code runs in WASM sandbox
- Explicit opt-in for capabilities
- HTTP requests only to approved domains
- Secrets never accessible from WASM code
- Timeout enforcement (default: 30s per tool execution)

#### FR-006: Signal Integration
**Description:** Signal messenger as primary communication channel  
**Acceptance Criteria:**
- [ ] Signal bot setup and pairing
- [ ] Receive and send messages
- [ ] Support for attachments (images, documents)
- [ ] Typing indicators
- [ ] Message threading

**Technical Details:**
- Reuse RustyTalon's Signal channel implementation
- Support device linking via QR code
- Store Signal credentials in system keychain
- Handle Signal protocol updates

---

### P1 (Should Have for Launch)

#### FR-007: Cost Tracking and Analytics
**Description:** Track and report API costs  
**Acceptance Criteria:**
- [ ] Per-message cost calculation
- [ ] Daily/weekly/monthly cost summaries
- [ ] Provider cost comparison
- [ ] Budget alerts
- [ ] Export to CSV/JSON

#### FR-008: Liquid AI Provider (Placeholder)
**Description:** Support for Liquid AI when API becomes available  
**Acceptance Criteria:**
- [ ] Placeholder implementation with NotImplemented error
- [ ] Documentation for adding real implementation
- [ ] Cost estimates based on announced pricing
- [ ] Ready to activate when API launches

#### FR-009: CLI Interface
**Description:** Interactive REPL for local usage  
**Acceptance Criteria:**
- [ ] Multi-line input support
- [ ] Command history
- [ ] Tab completion
- [ ] Syntax highlighting for code blocks
- [ ] Streaming response display

---

### P2 (Nice to Have)

#### FR-010: Web Dashboard
**Description:** Web UI for configuration and monitoring  
**Acceptance Criteria:**
- [ ] Cost analytics charts
- [ ] Provider performance comparison
- [ ] Conversation browser
- [ ] Settings editor
- [ ] Real-time status

#### FR-011: Additional Providers
**Description:** Support for more LLM providers  
**Acceptance Criteria:**
- [ ] Groq (Llama 3, Mixtral)
- [ ] Google (Gemini Pro, Ultra)
- [ ] Mistral AI
- [ ] Local Ollama support

#### FR-012: Advanced Routing
**Description:** More sophisticated routing strategies  
**Acceptance Criteria:**
- [ ] Response caching (exact query match)
- [ ] Hybrid provider responses (cheap generates, expensive refines)
- [ ] User preference learning
- [ ] Time-based routing (cheaper providers during off-peak)

---

## Non-Functional Requirements

### NFR-001: Performance
- **Latency:** <2s for simple queries, <10s for complex queries
- **Throughput:** Support 10 concurrent conversations
- **Memory:** <512MB RAM usage under normal load
- **Database:** Query response time <100ms for context retrieval

### NFR-002: Security
- **Authentication:** API keys stored in system keychain or Infisical
- **Sandbox:** All tool execution isolated in WASM
- **Network:** Outbound requests only to allowlisted domains
- **Secrets:** Never logged or exposed to tools
- **Audit:** All provider requests logged for review

### NFR-003: Reliability
- **Uptime:** 99%+ availability
- **Failover:** Automatic retry with exponential backoff
- **Error Handling:** Graceful degradation on provider failure
- **Data Persistence:** No message loss on crash
- **Recovery:** Automatic restart on failure

### NFR-004: Maintainability
- **Code Quality:** Pass `cargo clippy` with no warnings
- **Testing:** >80% code coverage
- **Documentation:** All public APIs documented
- **Logging:** Structured logging with tracing
- **Monitoring:** Metrics for provider health, costs, latency

---

## File Structure

```
rustytalon/
├── Cargo.toml
├── README.md
├── LICENSE
├── .env.example
├── docker-compose.yml
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── config/
│   │   ├── mod.rs
│   │   ├── settings.rs           # Settings struct and TOML parsing
│   │   ├── secrets.rs             # Infisical integration
│   │   └── providers.rs           # Provider configuration
│   ├── llm/
│   │   ├── mod.rs
│   │   ├── provider.rs            # LlmProvider trait
│   │   ├── factory.rs             # Provider factory
│   │   ├── providers/
│   │   │   ├── mod.rs
│   │   │   ├── anthropic.rs       # Anthropic implementation
│   │   │   ├── openai.rs          # OpenAI implementation
│   │   │   ├── liquid.rs          # Liquid AI placeholder
│   │   │   └── tests.rs           # Provider tests
│   │   └── routing/
│   │       ├── mod.rs
│   │       ├── analyzer.rs        # Complexity analyzer
│   │       ├── router.rs          # Smart router
│   │       └── tests.rs           # Routing tests
│   ├── memory/
│   │   ├── mod.rs
│   │   ├── database.rs            # PostgreSQL connection
│   │   ├── embeddings.rs          # Vector embeddings
│   │   ├── retrieval.rs           # Context retrieval
│   │   └── migrations/            # SQL migrations
│   ├── sandbox/
│   │   ├── mod.rs                 # WASM sandbox (from RustyTalon)
│   │   ├── capabilities.rs        # Permission system
│   │   └── tools.rs               # Tool execution
│   ├── channels/
│   │   ├── mod.rs
│   │   ├── signal.rs              # Signal integration (from RustyTalon)
│   │   ├── telegram.rs            # Telegram (optional)
│   │   ├── cli.rs                 # CLI/REPL
│   │   └── http.rs                # HTTP API
│   ├── onboard.rs                 # Setup wizard
│   └── error.rs                   # Error types
├── tests/
│   ├── integration_test.rs
│   ├── routing_test.rs
│   └── provider_test.rs
└── docs/
    ├── ARCHITECTURE.md
    ├── ROUTING.md
    ├── PROVIDERS.md
    ├── DEPLOYMENT.md
    └── CONTRIBUTING.md
```

---

## Data Models

### Core Types

```rust
// src/llm/provider.rs
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, request: CompletionRequest) 
        -> Result<CompletionResponse, LlmError>;
    
    async fn complete_with_tools(&self, request: ToolCompletionRequest) 
        -> Result<ToolCompletionResponse, LlmError>;
    
    fn model_name(&self) -> &str;
    fn cost_per_token(&self) -> (f64, f64); // (input, output)
    fn supports_streaming(&self) -> bool;
    fn supports_tools(&self) -> bool;
}

#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub messages: Vec<Message>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub tools: Vec<Tool>,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub content: String,
    pub provider_used: String,
    pub model_used: String,
    pub tokens: TokenUsage,
    pub cost: CostInfo,
    pub finish_reason: String,
}

#[derive(Debug, Clone)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct CostInfo {
    pub input_cost: f64,
    pub output_cost: f64,
    pub total_cost: f64,
}

#[derive(Debug)]
pub enum LlmError {
    ApiError(String),
    RateLimitError,
    NetworkError(String),
    NotImplemented(String),
    MissingApiKey(&'static str),
    UnknownProvider(String),
}
```

```rust
// src/config/providers.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub primary_provider: String,
    pub fallback_provider: String,
    pub routing_strategy: RoutingStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RoutingStrategy {
    CostOptimized,
    QualityFirst,
    Balanced,
}

#[derive(Debug, Clone)]
pub struct ApiKeys {
    pub anthropic: Option<String>,
    pub openai: Option<String>,
    pub liquid_ai: Option<String>,
    pub groq: Option<String>,
}
```

```rust
// src/llm/routing/analyzer.rs
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MessageComplexity {
    Simple,
    Medium,
    Complex,
}

impl MessageComplexity {
    pub fn from_message(text: &str, has_tools: bool) -> Self {
        if has_tools {
            return Self::Complex;
        }
        
        let token_count = text.split_whitespace().count();
        let has_code = text.contains("```");
        let has_complex_keywords = text.to_lowercase()
            .split_whitespace()
            .any(|w| matches!(w, "analyze" | "explain" | "debug" | "implement"));
        
        match (token_count, has_code, has_complex_keywords) {
            (_, true, _) | (200.., _, _) => Self::Complex,
            (50.., _, true) | (50.., _, _) => Self::Medium,
            _ => Self::Simple,
        }
    }
}
```

---

## API Specifications

### Provider Factory API

```rust
// Usage example
let factory = ProviderFactory::new(api_keys);

let provider = factory.create("anthropic/claude-sonnet-4-5")?;
// Returns: Box<dyn LlmProvider>

// Supported formats:
// - "anthropic/claude-opus-4-5"
// - "anthropic/claude-sonnet-4-5"
// - "anthropic/claude-haiku-4-5"
// - "openai/gpt-4o"
// - "openai/gpt-4o-mini"
// - "liquid/lfm-40b" (placeholder)
```

### Smart Router API

```rust
// Usage example
let primary = factory.create(&config.primary_provider)?;
let fallback = factory.create(&config.fallback_provider)?;

let router = SmartRouter::new(
    primary,
    fallback,
    config.routing_strategy,
);

let request = CompletionRequest {
    messages: vec![Message {
        role: "user".to_string(),
        content: "Explain quantum computing".to_string(),
    }],
    max_tokens: Some(4096),
    temperature: Some(0.7),
    tools: vec![],
};

let response = router.route_request(request).await?;
println!("Used: {}", response.provider_used);
println!("Cost: ${:.4}", response.cost.total_cost);
```

---

## Testing Requirements

### Unit Tests

**Coverage Target:** 80%+

**Test Files:**
- `src/llm/providers/tests.rs` - Test each provider implementation
- `src/llm/routing/tests.rs` - Test routing logic and complexity analysis
- `src/config/tests.rs` - Test configuration parsing and validation
- `src/memory/tests.rs` - Test database operations and vector search

**Example Test:**
```rust
#[tokio::test]
async fn test_anthropic_provider_simple_query() {
    let provider = AnthropicProvider::new(
        get_test_api_key(),
        "claude-haiku-4-5".to_string(),
    );
    
    let request = CompletionRequest {
        messages: vec![Message {
            role: "user".to_string(),
            content: "Say 'test' and nothing else".to_string(),
        }],
        max_tokens: Some(10),
        temperature: Some(0.0),
        tools: vec![],
    };
    
    let response = provider.complete(request).await.unwrap();
    
    assert!(response.content.contains("test"));
    assert_eq!(response.provider_used, "anthropic");
    assert!(response.cost.total_cost > 0.0);
    assert!(response.tokens.input_tokens > 0);
}
```

### Integration Tests

**Test Scenarios:**
1. **Full conversation flow** - User sends message → router selects provider → response returned
2. **Provider failover** - Primary fails → automatically switches to fallback
3. **Cost tracking** - Verify costs calculated correctly across providers
4. **Complexity routing** - Simple queries use cheap model, complex use expensive
5. **Database persistence** - Messages saved and retrievable
6. **Signal integration** - Send/receive messages via Signal

**Example:**
```rust
#[tokio::test]
async fn test_routing_escalation() {
    let config = load_test_config();
    let mut talon = RustyTalon::new(config).await.unwrap();
    
    // Simple query should use primary
    let response1 = talon.send("Hello").await.unwrap();
    assert_eq!(response1.provider_used, "liquid/lfm-40b");
    
    // Complex query should use fallback
    let response2 = talon.send(
        "Write a Rust async function to scrape a website with error handling"
    ).await.unwrap();
    assert_eq!(response2.provider_used, "anthropic/claude-sonnet-4-5");
}
```

### Performance Tests

```rust
#[tokio::test]
async fn test_concurrent_requests() {
    let router = create_test_router();
    
    let mut handles = vec![];
    for i in 0..10 {
        let router = router.clone();
        let handle = tokio::spawn(async move {
            router.route_request(simple_request(&format!("Query {}", i)))
                .await
                .unwrap()
        });
        handles.push(handle);
    }
    
    let results = futures::future::join_all(handles).await;
    assert_eq!(results.len(), 10);
    
    // All should complete in reasonable time
    // Total time < 30s for 10 concurrent requests
}
```

---

## Deployment Specifications

### System Requirements

**Minimum:**
- CPU: 2 cores
- RAM: 2GB
- Storage: 10GB
- PostgreSQL 15+
- Rust 1.85+

**Recommended (Homelab):**
- CPU: 4 cores
- RAM: 4GB
- Storage: 50GB SSD
- PostgreSQL 16+ with pgvector
- Rust 1.85+

### Environment Variables

```bash
# Required
DATABASE_URL=postgresql://user:pass@localhost/rustytalon
INFISICAL_CLIENT_ID=your_client_id
INFISICAL_CLIENT_SECRET=your_client_secret
INFISICAL_PROJECT_ID=your_project_id

# Optional
RUST_LOG=info
INFISICAL_ENV=production
```

### Docker Deployment

```yaml
# docker-compose.yml
version: '3.8'

services:
  rustytalon:
    build: .
    container_name: rustytalon
    environment:
      - DATABASE_URL=postgresql://postgres:${POSTGRES_PASSWORD}@db:5432/rustytalon
      - INFISICAL_CLIENT_ID=${INFISICAL_CLIENT_ID}
      - INFISICAL_CLIENT_SECRET=${INFISICAL_CLIENT_SECRET}
      - INFISICAL_PROJECT_ID=${INFISICAL_PROJECT_ID}
      - RUST_LOG=info
    depends_on:
      - db
    volumes:
      - ./data:/data
    restart: unless-stopped
    networks:
      - rustytalon-net

  db:
    image: pgvector/pgvector:pg16
    container_name: rustytalon-db
    environment:
      - POSTGRES_PASSWORD=${POSTGRES_PASSWORD}
      - POSTGRES_DB=rustytalon
    volumes:
      - postgres-data:/var/lib/postgresql/data
    restart: unless-stopped
    networks:
      - rustytalon-net

volumes:
  postgres-data:

networks:
  rustytalon-net:
```

### Infisical Secret Structure

```
Project: RustyTalon
Environment: production

Secrets:
- ANTHROPIC_API_KEY: sk-ant-...
- OPENAI_API_KEY: sk-...
- LIQUID_AI_API_KEY: (placeholder)
- GROQ_API_KEY: (optional)
- POSTGRES_PASSWORD: ...
```

---

## Migration from RustyTalon

### Code Preservation

**Keep from RustyTalon:**
- ✅ WASM sandbox implementation (`src/sandbox/`)
- ✅ Signal channel (`src/channels/signal.rs`)
- ✅ Database schema and migrations
- ✅ Security features (credential injection, endpoint allowlisting)
- ✅ Tool execution framework

**Remove from RustyTalon:**
- ❌ NEAR AI client code
- ❌ NEAR AI authentication
- ❌ NEAR AI-specific configuration

**Modify from RustyTalon:**
- 🔄 `src/llm/` - Replace NEAR AI provider with multi-provider system
- 🔄 `src/config/` - New configuration schema
- 🔄 `src/onboard.rs` - New setup wizard
- 🔄 `Cargo.toml` - Different dependencies

### Data Migration

**Database Schema:**
- Add `provider_used` column to messages table
- Add `tokens_input`, `tokens_output`, `cost_usd` columns
- Keep existing conversation and message tables

**Settings Migration:**
```bash
# Old RustyTalon settings
~/.rustytalon/settings.toml

# New RustyTalon settings
~/.rustytalon/settings.toml

# Migration script will:
# 1. Copy database URL
# 2. Prompt for new API keys
# 3. Set default routing strategy
```

---

## Success Metrics

### Launch Metrics (Week 6)

- [ ] All P0 features implemented and tested
- [ ] Can route between Anthropic and OpenAI providers
- [ ] Signal integration working
- [ ] Running in homelab with >95% uptime
- [ ] Documentation complete
- [ ] Cost < $20/month (vs current OpenClaw setup)

### Month 1 Metrics

- [ ] 30 days continuous uptime
- [ ] <5 bugs reported
- [ ] Average response latency <3s
- [ ] 80%+ queries handled by cheaper provider
- [ ] Cost savings >50% vs using only Claude Sonnet

### Month 3 Metrics

- [ ] 10+ GitHub stars
- [ ] 3+ external contributors
- [ ] All P1 features implemented
- [ ] Community documentation contributions
- [ ] Featured on Rust ML newsletter

---

## Risk Assessment

### Technical Risks

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Anthropic SDK breaking changes | Medium | High | Pin versions, monitor releases |
| PostgreSQL connection issues | Low | Medium | Connection pooling, retry logic |
| WASM sandbox vulnerabilities | Low | Critical | Regular security audits, upstream monitoring |
| Provider API rate limits | Medium | Medium | Exponential backoff, rate limit tracking |

### Business Risks

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Provider pricing changes | High | Medium | Multi-provider support enables switching |
| Liquid AI doesn't launch API | Medium | Low | Have Anthropic/OpenAI as alternatives |
| Low adoption | Medium | Low | Focus on personal use, homelab community |
| RustyTalon adds multi-provider | Low | Medium | We're first to market, better UX |

---

## Development Timeline

### Week 1: Foundation
- [ ] Fork RustyTalon repo
- [ ] Rebrand to RustyTalon
- [ ] Remove NEAR AI dependencies
- [ ] Setup development environment
- [ ] Database schema updates

### Week 2: Provider System
- [ ] Implement Anthropic provider
- [ ] Implement OpenAI provider
- [ ] Create provider factory
- [ ] Write provider tests
- [ ] Basic routing (no intelligence yet)

### Week 3: Smart Routing
- [ ] Complexity analyzer
- [ ] Routing strategies
- [ ] Quality checking
- [ ] Cost tracking
- [ ] Integration tests

### Week 4: Configuration & Setup
- [ ] Infisical integration
- [ ] Settings management
- [ ] Onboarding wizard
- [ ] Environment validation
- [ ] Migration tools

### Week 5: Testing & Polish
- [ ] Full test suite
- [ ] Performance testing
- [ ] Security audit
- [ ] Documentation
- [ ] Bug fixes

### Week 6: Deployment
- [ ] Docker setup
- [ ] Homelab deployment
- [ ] Signal integration testing
- [ ] Production monitoring
- [ ] Launch!

---

## Open Questions for Claude Opus

1. **Provider Abstraction:** Should we support streaming responses in the initial MVP or defer to v1.1?
2. **Caching:** Should we implement response caching at the router level or database level?
3. **Error Recovery:** How aggressive should automatic retries be? (Current thinking: 3 retries with exponential backoff)
4. **Quality Metrics:** What heuristics should determine if a response quality is "sufficient" for cost-optimized routing?
5. **Vector Embeddings:** Which embedding model should we use for semantic search? (OpenAI text-embedding-3-small vs. local model)
6. **Tool Permissions:** Should we preserve RustyTalon's exact WASM sandbox implementation or enhance it?
7. **Monitoring:** Should we integrate with Cloudflare AI Gateway from day 1 or add later?

---

## Appendix A: Provider Specifications

### Anthropic (Claude)

**Models:**
- `claude-opus-4-5` - $15/$75 per 1M tokens
- `claude-sonnet-4-5` - $3/$15 per 1M tokens  
- `claude-haiku-4-5` - $0.80/$4 per 1M tokens

**Features:**
- ✅ Function calling
- ✅ Streaming
- ✅ System prompts
- ✅ Vision (Opus/Sonnet)
- ❌ JSON mode

**Rate Limits:**
- Tier 1: 50 req/min, 40k tokens/min
- Tier 2: 1000 req/min, 80k tokens/min

### OpenAI (GPT)

**Models:**
- `gpt-4o` - $5/$15 per 1M tokens
- `gpt-4o-mini` - $0.15/$0.60 per 1M tokens

**Features:**
- ✅ Function calling
- ✅ Streaming
- ✅ JSON mode
- ✅ Vision
- ✅ Structured outputs

**Rate Limits:**
- Tier 1: 500 req/min, 30k tokens/min
- Tier 2: 5000 req/min, 450k tokens/min

### Liquid AI (Placeholder)

**Models:**
- `lfm-40b` - Estimated $0.50/$2 per 1M tokens (unconfirmed)

**Features:**
- ❓ Function calling (TBD)
- ❓ Streaming (TBD)
- ✅ Long context (32k+)
- ❌ Vision

**Status:** API not yet public

---

## Appendix B: Example Configurations

### Conservative (Quality First)
```toml
[providers]
primary = "anthropic/claude-sonnet-4-5"
fallback = "anthropic/claude-opus-4-5"
routing_strategy = "QualityFirst"
```

### Aggressive Cost Savings
```toml
[providers]
primary = "openai/gpt-4o-mini"
fallback = "anthropic/claude-haiku-4-5"
routing_strategy = "CostOptimized"
```

### Balanced (Recommended)
```toml
[providers]
primary = "liquid/lfm-40b"  # When available
fallback = "anthropic/claude-sonnet-4-5"
routing_strategy = "Balanced"
```

---

## Appendix C: Cost Comparison Examples

**Scenario:** 1000 messages/month, avg 500 tokens input, 1000 tokens output

| Strategy | Primary | Fallback | Est. Monthly Cost |
|----------|---------|----------|-------------------|
| Quality First | Sonnet | Opus | $22.50 (all Sonnet) |
| Cost Optimized | Mini | Sonnet | $2.25 (80% Mini, 20% Sonnet) |
| Balanced | Liquid | Sonnet | $3.75 (60% Liquid, 40% Sonnet) |
| Current OpenClaw | Sonnet | - | $22.50 (all Sonnet) |

**Savings:** 83-90% vs. current setup
