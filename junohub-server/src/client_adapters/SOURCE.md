# CC Switch source attribution

Source: https://github.com/farion1231/cc-switch
Revision: 7726c83476f9ae1f8a5b812aa844cd166339aa55
License: MIT, copied in junohub-server/docs/cc-switch-LICENSE.

Ported from `src-tauri/src/proxy`: error, SSE framing, canonical JSON, tool media, reasoning bridge, thinking and Copilot classification; Anthropic/Chat, Anthropic/Responses, Anthropic/Gemini, Codex/Chat and Codex/Anthropic transformations, tool context, schema/shadow stores and streaming state machines; Copilot, Codex OAuth and xAI OAuth account managers. Original module tests are retained.

Additional source: `proxy/usage/parser.rs`, `proxy/usage/calculator.rs` for token and Decimal cost semantics; common configuration merge/remove behavior from `services/provider/live.rs`.

Additional source: `services/codex_oauth_models.rs`, pure model-catalog builders from `codex_config.rs`, `model_capabilities.rs`, and the three bundled model catalog templates under `src/resources`. Native model-fetch and presets have separate existing attribution.

Adaptations: module paths use `client_adapters`; futures uses futures-util/futures-executor; logging uses tracing. `bridge.rs` connects the adapters to JunoHub's Axum helper. `client_accounts.rs` supplies web-safe device-flow sessions and local account binding. `client_codex_catalog.rs` uses JunoHub transactional writes and bundled templates, with host-bound official catalog selection. It preserves declarative source data in a catalog annotation for lossless native re-import.

Codex account credentials are isolated under `.junohub/managed-accounts`. The reference manager's automatic synchronization/deletion of official `.codex/auth.json` is deliberately removed, as the user requires official ChatGPT/Codex login to remain independent. Refresh generation and account deletion guards remain. Pure JWT identity parsing helpers are ported to `auth_identity.rs`; they are not a cryptographic JWT verification layer.

OAuth HTTP endpoint substitution is compiled only for tests and scoped to a Tokio task. Production builds use official endpoints. The usage-parser-dependent response-ID test asserts the transformed ID and input usage directly because JunoHub does not include CC Switch's usage repository. No Tauri service is started.
