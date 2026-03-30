# Courier

Just-in-time decryption delivery engine for ShrouDB.

## Identity

Courier is a **just-in-time decryption delivery engine** — it decrypts Cipher-encrypted recipients and message bodies at the moment of delivery, then immediately zeroizes all plaintext. Sensitive data is never stored in cleartext. Delivery channels (email via SMTP, webhooks via HTTP) are persisted in the Store. Callers render their own messages before submitting delivery requests.

ShrouDB is **not Redis**. It uses RESP3 as a wire protocol because RESP3 is efficient binary framing — not because ShrouDB is related to Redis in any way.

## Security posture

ShrouDB is security infrastructure. Every change must be evaluated through a security lens:

- **Fail closed, not open.** When in doubt, deny access, reject the request, or return an error. Never default to permissive behavior for convenience.
- **No plaintext at rest.** Recipients and sensitive message content are Cipher-encrypted. Plaintext exists only in memory during delivery.
- **Minimize exposure windows.** Plaintext in memory must be zeroized after use. Decrypted recipients are zeroized immediately after the delivery adapter call returns.
- **Cryptographic choices are not negotiable.** Do not downgrade algorithms, skip integrity checks, weaken key derivation, or reduce key sizes to simplify implementation.
- **Every shortcut is a vulnerability.** Skipping validation, hardcoding credentials, disabling TLS for testing, using `unsafe` without justification, suppressing security-relevant warnings — these are not acceptable trade-offs regardless of time pressure.
- **Audit surface changes require scrutiny.** Any change that modifies authentication, authorization, delivery adapters, decryption paths, or network-facing code must be reviewed with the assumption that an attacker will examine it.

## Pre-push checklist (mandatory — no exceptions)

Every check below **must** pass locally before pushing to any branch.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

### Rules

1. **Run all checks before every push.** No shortcuts.
2. **Pre-existing issues must be fixed.** If any check reveals issues — even if you didn't introduce them — fix them in the same changeset.
3. **Never suppress or bypass checks.** No `#[allow(...)]`, no `--no-verify`.
4. **Warnings are errors.** `RUSTFLAGS="-D warnings"` is set in CI.
5. **Dependency issues require resolution.** If `cargo deny` flags a new advisory, investigate and resolve it.
6. **Documentation must stay in sync.** Changes to commands, config, or public API must update docs.
7. **`protocol.toml` must stay in sync.** Changes to commands, parameters, or error codes must update `protocol.toml`.
8. **Cross-repo impact must be addressed.** Changes to shared types or protocols must update downstream repos.

## Architecture

```
shroudb-courier-core/        — domain types (Channel, DeliveryRequest, errors)
shroudb-courier-engine/      — Store-backed logic (CourierEngine, channel manager, delivery orchestration)
shroudb-courier-protocol/    — RESP3 command parsing + dispatch (Moat integration path)
shroudb-courier-server/      — TCP binary (standalone deployment) + adapter implementations
shroudb-courier-client/      — Rust client SDK
shroudb-courier-cli/         — CLI tool
```

## Dependencies

- **Upstream:** commons (shroudb-store, shroudb-storage, shroudb-crypto, shroudb-acl, shroudb-protocol-wire), shroudb-cipher-client
- **Downstream:** shroudb-moat (embeds engine + protocol), shroudb-codegen (reads `protocol.toml`)
