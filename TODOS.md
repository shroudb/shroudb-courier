# TODOS

## Debt

Each item below is captured as a FAILING test in this repo. The test is the forcing function — this file only indexes them. When a test goes green, check its item off or delete the entry.

Rules:
- Do NOT `#[ignore]` a debt test to make CI pass.
- A visible ratchet (`#[ignore = "DEBT-X: <reason>"]`) requires a matching line in this file AND a clear reason on the attribute. Use sparingly.
- `cargo test -p shroudb-courier-engine debt_` is the live punch list.

### Cross-cutting root causes

1. **Server binary hardcodes `None` for Sentry and Chronicle.** `main.rs:107` — default `PolicyMode::Closed` means every op fails-closed out of the box; server is unusable without config.
2. **Audit records `Ok` on failed deliveries.** `deliver()` records `EventResult::Ok` even when retry loop exhausts and receipt is `Failed`. Audit trail lies about delivery outcome.
3. **Every engine op hardcodes `None` for actor.** Same Sigil-shape gap.

### Open

- [x] **DEBT-1** — failed delivery must audit as `EventResult::Error`, not `Ok`. Test: `debt_1_failed_delivery_must_audit_as_error` @ `shroudb-courier-engine/src/engine.rs`.
- [x] **DEBT-2** — `deliver` audit must record caller actor (currently `"anonymous"`). Test: `debt_2_deliver_audit_must_record_caller_actor` @ same file.
- [x] **DEBT-3** — failed delivery audit must carry error metadata (currently metadata is empty). Test: `debt_3_failed_delivery_audit_must_carry_error_metadata` @ same file.
- [x] **DEBT-4** — `notify_event` must check a distinct policy action, not piggyback on `deliver`. Test: `debt_4_notify_event_must_check_distinct_policy_action` @ same file.
- [x] **DEBT-5** — `seed_channel` must emit a Chronicle event (currently bypasses both policy + audit). Test: `debt_5_seed_channel_must_emit_chronicle_event` @ same file.
- [x] **F-courier-6 (M)** — `channel_manager.rs:71,97,101` cache/store ordering is inconsistent; partial failure leaves cache/store divergent. Test: `debt_6_channel_mutation_must_persist_to_store_before_touching_cache` @ `shroudb-courier-engine/src/engine.rs`.
- [x] **F-courier-7 (L)** — `commands.rs:55` `ChannelList` and `Metrics` are `AclRequirement::None`; unauthenticated enumeration. Test: `debt_7_channel_list_and_metrics_must_not_be_public` @ `shroudb-courier-protocol/src/commands.rs`.
- [x] **F-courier-8** — `commands::parse_command` must accept keyword-arg forms for `CHANNEL CREATE` (`URL <default_recipient>`) and `DELIVER` (`SUBJECT`/`BODY`/`CONTENT_TYPE`). Legacy JSON-blob form preserved. Test: `debt_8_parse_command_must_accept_keyword_arg_forms` @ `shroudb-courier-protocol/src/commands.rs`.
