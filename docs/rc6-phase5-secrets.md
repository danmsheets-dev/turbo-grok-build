# Secrets Vault v1 (RC6 Phase 5)

## What shipped

### 1. Redaction guarantee on evidence (Auto Developer Log + Feature Request Log)

All write paths in `xai-grok-developer-log` now apply a defense-in-depth
`serde_json::Value` walk via `xai_grok_secrets::redact_json_string_values`
**in addition to** the existing per-field `redact_text` sanitization. This
ensures that no raw secret-shaped input can reach disk even if a future
field is added without its own field-level scrub.

**Files changed:**

- `crates/codegen/xai-grok-developer-log/src/store.rs`
  - `write_incident_at()` now serializes to `serde_json::Value`, runs
    `redact_json_string_values`, then writes pretty JSON to the temp file.
  - Added regression test `stored_incident_redacts_embedded_fake_tokens`:
    reports an incident with embedded `ghp_*` (GitHub PAT), `AKIA*` (AWS
    access key), and `Bearer sk-CANARY...` bearers; asserts the on-disk JSON
    contains `[REDACTED_SECRET]` and none of the fake token material.

- `crates/codegen/xai-grok-developer-log/src/feature_request/store.rs`
  - `write_at()` now runs `redact_json_string_values` on the serialized
    `serde_json::Value` before writing to disk (same defense-in-depth as ADL).
  - `sanitize_request_doc()` visibility bumped to `pub(super)` so the export
    module can call it.
  - Added regression test `stored_feature_request_redacts_embedded_fake_tokens`:
    same fake-token fixture shape, verifies `[REDACTED_SECRET]` in disk JSON.

- `crates/codegen/xai-grok-developer-log/src/feature_request/export.rs`
  - `export_feature_requests()` now calls `sanitize_request_doc()` on each
    loaded FR before serializing, then runs `redact_json_string_values` on the
    `serde_json::Value` before writing the evidence JSON. Previously this
    path wrote raw (un-sanitized) documents to the export pack.

### 2. Scoped credentials in auth.json (CredentialEntry)

Added a `CredentialEntry` type and optional `scopes` field to the
`xai-grok-auth` dependency-inversion seam (the crate that defines credential
types shared across shell, data-collector, and telemetry).

**Files changed:**

- `crates/codegen/xai-grok-auth/src/auth_provider.rs`
  - `CredentialEntry` struct: a serializable credential entry with optional
    `token` and `scopes` (`CredentialScopes = Option<Vec<String>>`).
    - `scopes: None` → legacy/unscoped (backward compatible, usable anywhere).
    - `scopes: Some([])` → explicitly unusable by scoped agents.
    - `scopes: Some(list)` → restricted to listed scopes.
  - Added `CredentialEntry::unscoped()`, `CredentialEntry::scoped()`, and
    `CredentialEntry::allowed_for(scope)` methods.
  - Added `scopes: CredentialScopes` field to `CredentialSnapshot` (the
    existing trait return type used by telemetry and 401-attribution),
    defaulting to `None` (fully backward compatible).
  - Serialization: `scopes` uses `#[serde(default, skip_serializing_if = "Option::is_none")]`,
    so legacy files with no `scopes` key round-trip unchanged.

- `crates/codegen/xai-grok-auth/src/lib.rs`
  - Re-exports `CredentialEntry`, `CredentialScopes`, and
    `credential_allowed_for`.

- `crates/codegen/xai-grok-auth/Cargo.toml`
  - Added `serde` (workspace) as a dependency.
  - Added `serde_json` (workspace) as a dev-dependency (for round-trip tests).

### 3. Scope enforcement helper

Added a pure function `credential_allowed_for(entry, requested_scope) -> bool`
in `xai-grok-auth`. This is the V1 helper for scope gating.

**Files changed:**

- `crates/codegen/xai-grok-auth/src/auth_provider.rs`
  - `pub fn credential_allowed_for(entry: &CredentialEntry, requested_scope: &str) -> bool`
  - Delegates to `CredentialEntry::allowed_for()`.

**MCP env-injection scope enforcement:** The `xai-grok-shell` crate (which
contains `extensions/mcp.rs` and `auth/storage.rs`) is **outside** the V1
allowed paths for this worktree. The MCP env-injection site in
`extensions/mcp.rs` reads env vars from `config.toml` `McpServerStdio`
definitions (not from auth.json credential scopes directly). The real
credential-to-env injection for MCP servers lives in
`xai-grok-shell/src/session/mcp_servers.rs` / `inner::start_mcp_servers`,
also outside allowed paths.

Per the requirement, enforcement is left as a documented TODO: the
`credential_allowed_for` helper is exported and ready. Applying a guard
clause at the actual call site in `xai-grok-shell` is deferred to a V2
slice that can edit those files. The helper is designed for exactly that
one-line guard: `if !credential_allowed_for(&entry, "mcp:sentry") { skip }`.

## What is deferred (V2)

- **TTL enforcement**: `CredentialEntry` has no expiry field yet. V2 will
  add `expires_at` and reject expired credentials.
- **Vault / AWS Secrets Manager JIT fetch**: credentials are still
  statically stored in `auth.json`. V2 will add just-in-time fetch from
  a managed secret store.
- **`/secret get`**: the CLI/`grok` agent surface for on-demand secret
  retrieval is not part of V1.
- **Live scope enforcement at MCP env injection**: as documented above, the
  guard clause in `xai-grok-shell` is deferred (outside allowed paths).

## Test evidence

```
$ cargo test -p xai-grok-secrets --lib -- --test-threads=2
running 16 tests
test sanitizer::tests::does_not_over_redact_sk_lookalikes ... ok
test sanitizer::tests::leaves_unrelated_strings_alone ... ok
test sanitizer::tests::match_any_count_matches_redact_secrets_passes ... ok
test sanitizer::tests::no_match_returns_borrowed ... ok
test sanitizer::tests::redact_url_strips_credentials_and_fragment ... ok
test sanitizer::tests::redact_user_paths_backstop_anonymizes_when_env_unset ... ok
test sanitizer::tests::redact_user_paths_backstop_skipped_when_env_known ... ok
test sanitizer::tests::redact_user_paths_collapses_home_and_username_before_punctuation ... ok
test sanitizer::tests::redact_user_paths_collapses_home_and_username_segments ... ok
test sanitizer::tests::redact_user_paths_home_prefix_matches_whole_segment_only ... ok
test sanitizer::tests::redacts_bare_jwt_leaving_no_token ... ok
test sanitizer::tests::redacts_additional_provider_prefixes ... ok
test sanitizer::tests::redacts_known_secret_shapes ... ok
test sanitizer::tests::redacts_pem_private_key_block ... ok
test sanitizer::tests::redacts_sensitive_url_query_params ... ok
test sanitizer::tests::url_regex_excludes_trailing_punctuation ... ok
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s

$ cargo test -p xai-grok-auth --lib -- --test-threads=4
running 9 tests
test auth_provider::tests::empty_scopes_blocks_all ... ok
test auth_provider::tests::empty_scopes_round_trips_as_empty_array ... ok
test auth_provider::tests::legacy_json_round_trips_without_scopes_field ... ok
test auth_provider::tests::credential_allowed_for_matches_entry_helper ... ok
test auth_provider::tests::scoped_entry_allows_only_listed ... ok
test auth_provider::tests::scoped_entry_round_trips ... ok
test auth_provider::tests::snapshot_scopes_default_none ... ok
test auth_provider::tests::unscoped_entry_allows_any_scope ... ok
test bearer_fragment::tests::bearer_suffix_semantics ... ok
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ cargo test -p xai-grok-developer-log --lib -- --test-threads=4
running 22 tests
test feature_request::fingerprint::tests::different_titles_differ ... ok
test detectors::tests::dispose_with_snapshot_is_silent ... ok
test feature_request::fingerprint::tests::explicit_fingerprint_normalized ... ok
test feature_request::fingerprint::tests::provider_included_for_provider_class ... ok
test feature_request::fingerprint::tests::same_class_components_title_same_fp ... ok
test feature_request::store::tests::rejects_empty_title ... ok
test detectors::tests::dispose_without_artifacts_files_incident ... ok
test feature_request::store::tests::report_dedups_by_fingerprint ... ok
test export::tests::export_writes_expected_files ... ok
test fingerprint::tests::provider_included_for_provider_errors ... ok
test fingerprint::tests::same_class_and_components_same_fingerprint ... ok
test feature_request::store::tests::stored_feature_request_redacts_embedded_fake_tokens ... ok
test fingerprint::tests::explicit_fingerprint_normalized ... ok
test feature_request::store::tests::list_filters_open_by_default ... ok
test redact::tests::redacts_bearer_tokens ... ok
test store::tests::rejects_empty_title ... ok
test redact::tests::truncates_long_title ... ok
test redact::tests::sanitize_evidence_redacts_snapshot_ref ... ok
test store::tests::root_override_takes_precedence ... ok
test store::tests::resolve_records_proving_sha ... ok
test store::tests::report_creates_and_dedups ... ok
test store::tests::stored_incident_redacts_embedded_fake_tokens ... ok
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.18s

$ cargo check -p xai-grok-secrets -p xai-grok-auth -p xai-grok-developer-log
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 24.74s

$ cargo test -p xai-grok-auth --lib --features middleware -- --test-threads=4
running 15 tests
test auth_provider::tests::legacy_json_round_trips_without_scopes_field ... ok
test auth_provider::tests::credential_allowed_for_matches_entry_helper ... ok
test auth_provider::tests::empty_scopes_blocks_all ... ok
test auth_provider::tests::empty_scopes_round_trips_as_empty_array ... ok
test auth_provider::tests::scoped_entry_allows_only_listed ... ok
test auth_provider::tests::scoped_entry_round_trips ... ok
test auth_provider::tests::snapshot_scopes_default_none ... ok
test auth_provider::tests::unscoped_entry_allows_any_scope ... ok
test bearer_fragment::tests::bearer_suffix_semantics ... ok
test retry_middleware::tests::test_401_no_refresh_returns_401 ... ok
test retry_middleware::tests::execute_with_stamp_is_none_when_nothing_stamped ... ok
test retry_middleware::tests::execute_with_stamp_reports_last_stamped_bearer ... ok
test retry_middleware::tests::test_e2e_auth_header_stamped_automatically ... ok
test retry_middleware::tests::test_e2e_stale_token_refreshed_and_retried ... ok
test retry_middleware::tests::test_max_retries_bounds_attempts ... ok
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
```

## Fixture note

All test fixtures use obviously-fake token material constructed at runtime by
joining string fragments (e.g. `"ghp_f" + "akefakefakefakefakefakefake"`), so
no real-looking secret ever appears contiguously in source.
