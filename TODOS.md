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

- [ ] **DEBT-1** — failed delivery must audit as `EventResult::Error`, not `Ok`. Test: `debt_1_failed_delivery_must_audit_as_error` @ `shroudb-courier-engine/src/engine.rs`.
- [ ] **DEBT-2** — `deliver` audit must record caller actor (currently `"anonymous"`). Test: `debt_2_deliver_audit_must_record_caller_actor` @ same file.
- [ ] **DEBT-3** — failed delivery audit must carry error metadata (currently metadata is empty). Test: `debt_3_failed_delivery_audit_must_carry_error_metadata` @ same file.
- [ ] **DEBT-4** — `notify_event` must check a distinct policy action, not piggyback on `deliver`. Test: `debt_4_notify_event_must_check_distinct_policy_action` @ same file.
- [ ] **DEBT-5** — `seed_channel` must emit a Chronicle event (currently bypasses both policy + audit). Test: `debt_5_seed_channel_must_emit_chronicle_event` @ same file.
- [ ] **F-courier-6 (M)** — `channel_manager.rs:71,97,101` cache/store ordering is inconsistent; partial failure leaves cache/store divergent. *No debt test yet; add one before fixing.*
- [ ] **F-courier-7 (L)** — `commands.rs:55` `ChannelList` and `Metrics` are `AclRequirement::None`; unauthenticated enumeration. *No debt test yet; add one before fixing.*
