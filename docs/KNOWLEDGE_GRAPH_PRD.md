# PRD: Native Knowledge Graph for RustyTalon

**Status:** v1 implemented (Phase A + B); revised post-v1 based on real-world comparison to Amazon Quick — see §12
**Owner:** RustyTalon maintainers
**Component:** RustyTalon (self-hosted)
**Related infra:** Neo4j, Kimi K2 / Claude Sonnet routing (existing)

---

## 1. Problem

Every conversation with an AI assistant today starts from zero context. Amazon Quick's personal knowledge graph feature demonstrated the value of persistent, structured memory: it auto-extracts entities (people, projects, orgs, meetings) and typed relationships between them from connected sources, so the assistant doesn't need to be re-briefed every session.

RustyTalon currently has no structured, queryable memory of this kind. Context lives in whatever's in the current conversation window or gets manually re-explained. There's no way to ask "who's involved with X" or "what's the latest on project Y" and get an answer grounded in accumulated relationships rather than a fresh search.

Routing this through Claude's cloud-hosted MCP connectors was evaluated and rejected: it requires exposing the graph to the public internet, and the available auth mechanisms (IP allowlisting, header tokens) don't meet the bar for something holding potentially sensitive personal/professional relationship data. RustyTalon is already the trusted, self-hosted, publicly-reachable front door — the graph should live behind it, not behind a second exposed surface.

## 2. Goals

- RustyTalon can create, update, and query a structured graph of entities (people, projects, organizations, meetings, documents, topics) and typed relationships between them.
- The graph persists across sessions and compounds over time — it should get more useful the longer it's used, without manual graph maintenance.
- RustyTalon can use the graph automatically during normal conversation (e.g. "what's the status of the Legator flip" pulls from graph context) without the user having to invoke it explicitly.
- A background extraction routine populates the graph from RustyTalon's existing surfaces (Discord conversations, and eventually other connected sources) on a schedule, not just on-demand.
- The graph is queryable and inspectable outside of RustyTalon (Neo4j Browser) for debugging and manual curation.
- No new public-facing attack surface. The graph is reachable only from RustyTalon, on the internal Docker network.

## 3. Non-goals

- Not building a general-purpose graph database product — this is scoped to RustyTalon's own use.
- Not replicating Quick's OS-level file monitoring or its Slack/email auto-ingest out of the box. Source ingestion is added incrementally, starting with whatever RustyTalon already has access to.
- Not exposing the graph to Claude's cloud connectors or any other external MCP client in v1.
- Not solving multi-user access control — this is a single-user personal graph.

> **Revision note (post-v1):** The original v1 draft included a non-goal of "not building a general knowledge-graph UI," reasoning that Neo4j Browser was sufficient for inspection. That was written without direct experience of what makes a personal knowledge graph feel useful day-to-day. Having since used Amazon Quick's desktop knowledge graph viewer, the interactive, categorized, browsable visualization turned out to be a meaningfully valuable part of the feature on its own — not just a debugging aid — because it supports rediscovering forgotten connections by browsing, not just querying. That non-goal is retracted; a browsable graph visualization in RustyTalon's own web gateway is now in scope (see §12).

## 4. Users

Single user, via RustyTalon's existing interfaces (Discord, web UI). No other consumers in v1.

## 5. Requirements

### 5.1 Functional

| ID | Requirement |
|----|-------------|
| F1 | RustyTalon can create an entity node with a type (Person, Project, Organization, Meeting, Document, Topic) and properties. |
| F2 | RustyTalon can create a typed, directional relationship between two existing entities, with optional properties (e.g. `since`, `role`). |
| F3 | RustyTalon can search entities by name/fuzzy match and return matches with their immediate relationships (1-hop). |
| F4 | RustyTalon can traverse relationships N hops from a given entity (e.g. "everyone connected to Project X within 2 hops"). |
| F5 | RustyTalon can update an existing entity's properties without creating a duplicate node (dedup on name + type at minimum). |
| F6 | RustyTalon can delete or merge entities (for correcting bad extractions). |
| F7 | Graph read/write tools are exposed to RustyTalon's model routing so both Kimi K2 and Claude Sonnet (fallback) can call them during normal conversation. |
| F8 | A scheduled routine periodically reviews recent conversation history and extracts candidate entities/relationships, writing them to the graph. |
| F9 | Extracted entities/relationships from F8 are staged for review before being committed, at least during initial rollout (see 5.3). |
| F10 | User can ask RustyTalon in plain language about graph contents ("what do you know about X") and get an answer grounded in graph traversal, not just a text search of chat history. |

### 5.2 Non-functional

| ID | Requirement |
|----|-------------|
| N1 | Neo4j and RustyTalon communicate over the internal Docker network only (`kg-net` or equivalent) — no ports exposed beyond what's needed for Neo4j Browser access from the user's own devices. |
| N2 | Neo4j Browser (`:7474`) reachable only via WARP/Cloudflare Access private hostname, consistent with how other sensitive services are handled — never a public hostname. |
| N3 | Graph data persisted to disk, included in the existing backup routine. |
| N4 | Entity/relationship writes are idempotent enough that re-running extraction on the same content doesn't produce duplicate nodes. |
| N5 | Graph query latency shouldn't noticeably slow down normal RustyTalon conversation turns — target under ~200ms for typical 1-2 hop lookups. |
| N6 | Credentials (Neo4j password) stored the same way other RustyTalon secrets are (env var / existing secrets pattern), never hardcoded. |

### 5.3 Extraction & review (staged rollout)

Automatic extraction is the highest-risk part of this — bad extractions compound over time and pollute the graph. Rollout in two phases:

- **Phase A (manual only):** No scheduled extraction. The user explicitly tells RustyTalon "log this" after something notable (a meeting, a decision, a new contact). This validates the entity/relationship schema and tool behavior with zero risk of silent graph pollution.
- **Phase B (scheduled, reviewed):** A scheduled routine proposes extractions from recent conversation history but writes them to a staging area (a separate label, e.g. `:Candidate`, or a simple pending list) rather than committing directly. The user reviews and approves/rejects in a batch (e.g. a Discord message summarizing candidates with react-to-approve, or a short list in the web UI).
- **Phase C (scheduled, auto-commit) — stretch, not required for v1:** Once the extraction prompt has proven reliable over Phase B, allow high-confidence extractions to commit automatically, with low-confidence ones still staged.

v1 ships Phase A + B. Phase C is explicitly out of scope until extraction quality is validated.

## 6. Architecture

```
Discord / Web UI
      │
      ▼
 RustyTalon
   ├─ model routing (Kimi K2 / Claude Sonnet fallback)
   ├─ existing tools (web_search via SearXNG, etc.)
   └─ NEW: graph tools (neo4rs driver)
      │
      │  Bolt (internal network only)
      ▼
   Neo4j
      └─ persisted, backed up
```

- No `mcp-neo4j-memory` container. RustyTalon talks to Neo4j directly via `neo4rs`, since RustyTalon is Rust-native and the MCP translation layer only exists to serve non-native clients (like Claude's cloud, which is explicitly out of scope here).

## 7. Data model

Starter schema — expected to evolve, not a fixed contract:

**Entity types (node labels):** `Person`, `Project`, `Organization`, `Meeting`, `Document`, `Topic`

**Relationship types:**
- `(Person)-[:LEADS]->(Project)`
- `(Person)-[:MEMBER_OF]->(Organization)`
- `(Person)-[:ATTENDED]->(Meeting)`
- `(Person)-[:MENTIONED_IN]->(Document)`
- `(Project)-[:RELATES_TO]->(Project)`
- `(Document)-[:REFERENCES]->(Project)`
- `(Meeting)-[:ABOUT]->(Project)`

Relationships may carry properties where "how" matters (e.g. `[:LEADS {since: "2026-03"}]`).

## 8. Tool interface (exposed to RustyTalon's model routing)

| Tool | Purpose |
|------|---------|
| `create_entity(type, name, properties)` | Create or upsert an entity. |
| `create_relationship(from_entity, to_entity, type, properties)` | Create a typed edge between two entities. |
| `search_entities(query)` | Fuzzy search by name, returns matches + 1-hop context. |
| `get_entity_context(name, hops=2)` | Full traversal from a given entity out to N hops. |
| `update_entity(name, properties)` | Update properties on an existing entity without duplicating it. |
| `stage_candidate(type, entities, relationships, source)` | Write to the review staging area (Phase B). |
| `list_candidates()` / `approve_candidate(id)` / `reject_candidate(id)` | Review workflow for staged extractions. |

## 9. Milestones

1. **M1 — Infra:** Neo4j deployed per the compose file, backups confirmed working, Neo4j Browser reachable privately.
2. **M2 — Native driver integration:** `neo4rs` wired into RustyTalon, basic `create_entity` / `create_relationship` / `search_entities` tools working, tested manually via Discord ("log this: ...").
3. **M3 — Traversal & context tools:** `get_entity_context` implemented, RustyTalon uses graph context automatically in relevant conversations (F10).
4. **M4 — Phase B extraction:** Scheduled routine + staging/review workflow implemented and running for at least 2 weeks before considering Phase C.
5. **M5 — Schema refinement:** Revisit entity/relationship types based on what Phase A/B actually produced; adjust before investing further.

## 10. Success metrics

- Qualitative, primarily: does RustyTalon meaningfully reduce "let me re-explain who X is" moments in normal use.
- Graph grows without manual pruning becoming a chore (a proxy for extraction quality — if the user is rejecting most candidates in Phase B review, the extraction prompt needs work before Phase C is even considered).
- No graph-related latency complaints in normal conversation.

## 11. Risks / open questions

- **Extraction quality is unproven.** This is the single biggest risk to the whole feature being useful vs. becoming a junk drawer. Staged rollout (Section 5.3) exists specifically to de-risk this.
- **Schema rigidity vs. flexibility tradeoff.** Starting schema is intentionally loose; over-designing it now before real data exists would be premature.
- **Dedup/entity resolution.** "Sanjay" vs. "Sanjay Mehta" vs. a nickname — needs a matching strategy (exact match to start, fuzzy/embedding-based later if needed).
- **Backup/restore of graph state not yet tested end-to-end** — should be validated as part of M1, not assumed to work because the existing backup routine already covers the volume path.
- **Future external access.** If a future need arises to query the graph from Claude's cloud or another external client, that reopens the exposure question addressed in Section 1 — deliberately deferred, not solved here.

---

## Implementation status (RustyTalon, `src/graph/`, `src/tools/builtin/graph.rs`)

v1 (Phase A + B) is implemented, gated behind the optional `neo4j` Cargo feature:

- **M2 done:** `create_entity`, `update_entity`, `create_relationship`, `search_entities` tools implemented and callable.
- **M3 partially done:** `get_entity_context` (traversal) is implemented and callable, satisfying the traversal half of M3. However, **F10's "automatic" requirement is not actually met** — there is no proactive/ambient injection of graph context into conversation turns. The tool exists but is only ever invoked if the LLM's own tool-selection logic happens to decide to call it for a given message; nothing guarantees graph context surfaces "automatically" the way F10 and M3 originally specified. Earlier revisions of this document marked M3 as fully "done" based on tool availability alone, without verifying the automatic-use behavior — that was a self-graded status error, corrected here. True ambient retrieval (detecting entity mentions pre-turn and injecting relevant graph context without an explicit tool call) remains unbuilt and is now tracked as its own milestone (see §12).
- **F6 done:** `delete_entity`, `merge_entities` (requires the Neo4j APOC plugin).
- **M4 done:** `stage_candidate`, `list_candidates`, `approve_candidate`, `reject_candidate` — candidates are stored as `:GraphCandidate` nodes in Neo4j itself (not a new `Database`-trait table), and review happens through tool calls from any channel rather than a dedicated Discord-react or web UI (open question left unresolved above; MVP took the channel-agnostic tool-call route instead).
- **F8's "scheduled routine"** has no dedicated built-in scheduler and, as of this writing, no routine has actually been created — create one via the existing `routine_create` tool (a `FullJob` whose description tells the agent to review conversation history and call `stage_candidate`), reusing the general-purpose routine engine instead of adding graph-specific scheduling code. Until this routine is created, the graph has no ingestion path at all and will remain empty.
- **M1 (infra)** and **M5 (schema refinement post-usage)** are deployment-time/operational, outside this repo's code.
- **Phase C (auto-commit)** is not built, per the PRD's explicit deferral.
- **No visualization UI exists** (see retracted non-goal above and §12).

See `CLAUDE.md`'s "Knowledge Graph" section for the current developer-facing reference (tool list, config, injection-safety notes, APOC requirement).

## 12. Post-v1 additions (informed by real Amazon Quick usage)

These were not part of the original v1 scope. They're added here because direct experience using a comparable shipped product (Amazon Quick's personal knowledge graph) surfaced value that the original agent-authored draft didn't anticipate or actively scoped out.

| ID | Requirement | Status |
|----|-------------|--------|
| F11 | A scheduled extraction routine actually exists and runs (F8 is implemented in principle but no routine has been created) — needed before any of the below can be evaluated with real data. | Not started |
| F12 | RustyTalon's web gateway includes a graph visualization panel: force-directed layout, node color by entity category, node size by connectivity (e.g. degree or PageRank-equivalent), zoom/pan, entity search, focus-on-node, and a "browse all entities by category with edge counts" sidebar — modeled on Quick's knowledge graph viewer. | Not started |
| F13 | A `graph_stats` capability (node/edge/entity/candidate counts) backs both the visualization panel's header and can be surfaced in chat. | Not started |
| F14 | Ambient graph context injection: before an LLM turn, detect likely entity mentions in the incoming message and pull relevant `get_entity_context` results into the prompt automatically, rather than relying solely on the model's own tool-selection judgement. Must be introduced only after F11 has run long enough to validate extraction quality — ambient injection of untrustworthy graph data is worse than no graph context at all. | Not started |
| F15 (stretch) | Confidence-scored auto-commit for high-confidence candidates (the PRD's original Phase C), informed by real approve/reject ratios collected from F11's review process. | Not started, blocked on F11 data |

Recommended build order: F11 → F12 → F13 → F14 → F15. F12 (visualization) is deliberately sequenced before F14 (ambient retrieval) because a browsable UI is safe to ship against a small/imperfect graph, while ambient injection actively degrades the assistant if the underlying data is still noisy.
