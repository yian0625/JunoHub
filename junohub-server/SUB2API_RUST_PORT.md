# Sub2API Rust Compatibility Port

## Baseline

- Upstream: `https://github.com/Wei-Shaw/sub2api`
- Version: `v0.1.182`
- Commit: `5a7d469622911a6b1291a692376df5fa03f9ac2e`
- License: LGPL-3.0
- Inventory: 2,492 Go files, about 1,021,291 lines, 271 SQL migration files

The Go service was the production compatibility oracle until the Rust modules passed differential
tests. As of the cutover below, the Go container has been removed from the Compose stack and the
Rust account engine is the only runtime: it owns the PostgreSQL/Redis data plane
(`data/accounts/`), the scheduler, the sticky-session store and the gateway protocols that the
JunoHub product surface exercises.

## Cutover scope (what "complete" means here)

The JunoHub product surface—not the upstream admin UI—is the compatibility contract:

- Account creation via JunoHub console: browser PKCE OAuth for OpenAI Codex, Claude,
  Gemini, Grok and Antigravity; access/refresh-token import for non-OpenAI OAuth accounts;
  and API-key accounts for `openai / anthropic / gemini / grok / kimi / zhipu / deepseek`.
- Gateway protocols: OpenAI Chat Completions + Responses (Codex OAuth passthrough and
  conversion), generic OpenAI-compatible forwarding for API-key accounts, and
  Chat Completions ⇄ Anthropic Messages / Gemini generateContent conversion (request,
  response, SSE streaming and error-shape mapping).
- Scheduler: Redis snapshot buckets, outbox worker, concurrency slots, last_used writes and
  account_last_used outbox events (Go wire format).
- Sticky sessions: xxHash/SHA-256 session hashes, previous_response_id → account binding,
  response owner binding, dual legacy read/write.
- Codex family-alias fallback: `gpt-5.x` public aliases transparently retry as `gpt-5.x-sol`
  when the ChatGPT backend rejects them, echoing the requested model name (Go behavior).
- Codex SSE aggregation: `response.completed` carries an empty `output` array on the ChatGPT
  backend; terminal responses are assembled from `response.output_item.done` events.

Out of scope (upstream-only surface, no JunoHub product path): the Go admin web UI, automated
password/cookie login, WebSocket ingress, image generation endpoints,
channel monitors, billing/pricing tables, announcements and audit logs. The PostgreSQL schema
retains those tables (see `deploy/accounts-schema.sql`) so the data is never lost, but no Rust
code writes them.

## Module order

1. Accounts, groups and API key domain rules
2. PostgreSQL repositories and migration compatibility
3. Credential encryption and OAuth session persistence
4. OpenAI Codex OAuth, Session, PAT and refresh-token flows
5. Claude OAuth, setup token, API key, Bedrock and Vertex flows
6. Gemini OAuth, AI Studio and service-account flows
7. Grok OAuth, SSO, password and API-key flows
8. Antigravity and China-provider account flows
9. Model discovery and mapping
10. Scheduler, sticky sessions, concurrency and cooldowns
11. Gateway protocols and streaming used by JunoHub
12. BYOK usage accounting and background jobs
13. Shadow traffic and Go/Rust differential verification
14. Cutover and removal of the Go container

## Compatibility gates

A module is not complete until it preserves:

- HTTP status and response schema
- Database writes and transaction boundaries
- Token refresh and credential encryption behavior
- Streaming event order
- Scheduler selection and exclusion behavior
- Usage side effects for JunoHub BYOK requests (token counts are persisted in SQLite and
  PostgreSQL; monetary cost remains zero because JunoHub does not define a resale price)
- Existing PostgreSQL data

## Completion audit

Run:

```bash
./scripts/check-sub2api-rust-port.sh
./scripts/check-sub2api-rust-port.sh --require-complete
```

The strict command must fail while any Go adapter call, Go image reference, missing Rust Redis
dependency, or unchecked milestone remains.

## Differential verification (cutover gate)

Captured against the running Go gateway (`weishaw/sub2api:0.1.182`) before removal, then replayed
through the Rust engine with the same account pool and prompt:

- Go: `{"content":"GO-BASELINE-OK","model":"gpt-5.6","usage":{"prompt_tokens":16,"completion_tokens":10,"total_tokens":26},"id":"resp_…"}`
- Rust: identical content, model echo, usage and `resp_…` id shape (HTTP 200).

Also verified post-cutover: SSE chat streaming chunk order, live upstream model discovery,
scheduler outbox watermark advancement, sticky-session / response-owner Redis keys,
`accounts.last_used_at` updates, `account_last_used` outbox rows and BYOK token usage writes.

## Current progress

### 2026-09-10 compatibility audit corrections

Historical checked milestones below are not a full compatibility certification. The focused audit is recorded in `docs/sub2api-compatibility-audit-2026-09-10.md`.

- [x] Durable OAuth completion receipts: encrypted token continuation after provisioning failure, identity/state/target binding, concurrent claim exclusion and completed-result replay
- [x] Callback state validation and duplicate-submit guards; Gemini registration/onboarding branching and explicit unsupported-client rejection
- [ ] OAuth per-account generation tests and platform-correct model discovery connected to the console
- [ ] Account model mappings applied during forwarding and verified against Go
- [ ] Pre-refresh concurrency coordination and rotation-race regression tests
- [ ] Real OAuth browser-to-account regression after the authorization fixes (Google eligibility is not implied by local tests)
- [ ] Gemini/Antigravity tool-result names, thought/signature handling, thinking-token accounting, project backfill and reauthorization configuration preservation verified against the upstream behavior

- [x] Runtime service and data moved into the JunoHub Compose stack
- [x] Fixed source and license baseline
- [x] Rust account, group and API-key domain types
- [x] Domain validation and schedulability tests
- [x] Direct PostgreSQL connection from JunoHub Server
- [x] Read-only account, group, API-key and association repositories
- [x] Go/Rust row-count and field-decoding parity check
- [x] PostgreSQL transactional write probe for group, account, link and API key
- [x] Scheduler outbox parity for `group_changed` and `account_changed`
- [x] SHA-256 scheduler dedup-key compatibility
- [x] Real-database rollback parity with zero residual rows
- [x] Production write repository for atomic group/account/link/key creation
- [x] Compensating soft-delete path with scheduler invalidation
- [x] API-key account creation cut over from Go admin API to Rust repository
- [x] Rust Redis connection and startup health check
- [x] Scheduler account/meta payload decoding
- [x] Scheduler bucket and outbox watermark reads
- [x] Read-only scheduler eligibility and concurrency-load snapshots
- [x] Deterministic candidate ranking using Go default score weights
- [x] Atomic concurrency slot acquire/release semantics (config-aligned 30 minute TTL)
- [x] Live lease heartbeat for long-running SSE streams
- [x] User concurrency slots and API-key request tracking
- [x] Account/user wait queues and active-index maintenance
- [x] OpenAI xxHash/SHA-256 sticky-session key compatibility
- [x] Sticky session dual-read/write, refresh and delete semantics
- [x] Previous-response account and downstream-owner persistence
- [x] Responses session-window atomic claim/refresh/delete
- [x] Sticky and previous-response layers integrated into scheduler diagnostics
- [x] Scheduler snapshot epoch/version/ready/active state machine
- [x] Scheduler retire/reopen fencing and 60-second reader grace
- [x] Bucket rebuild and group-lifecycle lock ownership semantics
- [x] Scheduler outbox read-only audit
- [x] Scheduler outbox claim/dedup-release rollback probe
- [x] Scheduler outbox cleanup advisory-lock and age-window rollback probe
- [x] Rust scheduler worker enabled as the only scheduler runtime
- [x] Full canonical snapshot rebuild on isolated Redis database
- [x] Live provider model discovery with account failover and protocol-specific headers
- [x] OpenAI Codex OAuth Responses streaming and non-stream aggregation
- [x] OpenAI Chat Completions request/response/SSE bridge
- [x] OpenAI OAuth pre-refresh with PostgreSQL refresh-token CAS
- [x] OAuth providers
  - [x] OpenAI Codex PKCE session generation
  - [x] Per-JunoHub-user OAuth session ownership
  - [x] OpenAI token exchange request compatibility
  - [x] OpenAI ID-token claims mapping
  - [x] OAuth account, group and Key persistence transaction
  - [x] JunoHub provider and model-route provisioning
  - [x] Authorization-code acceptance exercised by the production account
  - [x] Claude browser PKCE OAuth, token exchange/import and refresh
  - [x] Gemini Code Assist, Google One and custom AI Studio OAuth clients
  - [x] Grok browser PKCE OAuth, token exchange/import and refresh
  - [x] Antigravity browser PKCE OAuth, token exchange/import and refresh
  - [x] OAuth callback pages for fixed localhost ports 1455, 8085 and 56121
  - [x] OAuth session encryption, persistence across restarts and atomic single-use consume
  - [x] OAuth/API credentials encrypted field-by-field in PostgreSQL, including legacy migration
- [x] Managed account pools
  - [x] Additional accounts of the same user/platform join one group and provider
  - [x] Provider deletion cascades through model routes, internal Key, group and accounts
- [x] Scheduler
  - [x] Gateway selection prefers previous_response_id and session-hash affinity
  - [x] Snapshot-ranked candidates with DB fallback when buckets are cold
  - [x] last_used DB/Redis writes plus account_last_used outbox events
  - [x] Retryable transport/status failures exclude the failed account and select another
  - [x] Rate-limit, overload and temporary cooldown state is persisted and cleared on recovery
- [x] Gateway protocol cutover
  - [x] Codex family-alias fallback (`gpt-5.x` → `gpt-5.x-sol`) with requested-model echo
  - [x] Codex SSE aggregation from `response.output_item.done` items
  - [x] Codex conversion drops `temperature` / `max_output_tokens` (upstream rejects them)
  - [x] Chat Completions ⇄ Anthropic Messages conversion (request/response/SSE/errors)
  - [x] Chat Completions ⇄ Gemini generateContent conversion (request/response/SSE/errors)
  - [x] Claude OAuth Bearer and required beta headers
  - [x] Gemini/Antigravity `v1internal` project request wrapping and response unwrapping
  - [x] Grok CLI gateway identity headers and Responses/Chat Completions forwarding
  - [x] Engine builds its own upstream headers (no client header leakage)
  - [x] sub2api providers never fall back to the removed Go base URL
  - [x] Existing provider rows migrated to the deployment-independent `managed:account-engine` marker at startup
- [x] Usage accounting
  - [x] JSON and SSE usage extraction with tokenizer fallback when upstream omits input usage
  - [x] Per-request and per-Key token totals in the JunoHub console
  - [x] Matching token rows in the account-engine `usage_logs` table
- [x] Go container and `sub2api/` directory removed; data relocated to `data/accounts/`
- [x] Fresh-install schema bootstrap via `deploy/accounts-schema.sql`
