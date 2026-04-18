# Courier — ShrouDB Repository Analysis

**Component:** Courier  
**Type:** Engine (multi-crate workspace: 4 libraries, 2 binaries)  
**Language:** Rust (edition 2024, MSRV 1.92)  
**License:** MIT OR Apache-2.0  
**Published:** Private "shroudb" registry (6 crates) + Docker Hub (2 images)  
**Analyzed:** /Users/nlucas/dev/shroudb/shroudb-courier

---

## Role in Platform

Courier is the just-in-time decryption delivery engine for ShrouDB. It bridges encrypted data (Cipher-encrypted recipients and message bodies) with external delivery channels (SMTP email, HTTP webhooks), decrypting only at the moment of delivery and immediately zeroizing all plaintext from memory. Without Courier, ShrouDB has no way to send notifications or alerts from encrypted data — other engines (Cipher key rotation, Forge cert expiry) depend on it via `notify_event` for operational alerting.

---

## Behavioral Surface

### Public API

**RESP3 Commands (10):**
- `AUTH <token>` — authenticate connection
- `CHANNEL CREATE <name> <type> <config_json>` — create delivery channel
- `CHANNEL GET <name>` — get channel config
- `CHANNEL LIST` — list all channels
- `CHANNEL DELETE <name>` — delete channel
- `DELIVER <json>` — decrypt recipient + body, deliver via adapter
- `NOTIFY_EVENT <channel> <subject> <body>` — deliver to channel's default_recipient
- `HEALTH` — health check (returns channel count)
- `PING` — connectivity check
- `COMMAND LIST` — list available commands

**Key Traits:**
- `Decryptor` (engine/capabilities.rs) — pluggable decryption; production impl connects to Cipher server
- `DeliveryAdapter` (engine/capabilities.rs) — pluggable delivery; implementations for SMTP (lettre) and webhook (reqwest)
- `CourierOps` (core/ops.rs) — `fn notify(&self, channel, subject, body)` — cross-engine notification interface

**Client SDK:** `CourierClient` with `connect`, `auth`, `health`, `channel_create/get/list/delete`, `deliver`

**CLI:** Interactive REPL mode or single-command batch execution

### Core operations traced

**1. DELIVER flow** (engine.rs:150 → delivery.rs:12):
1. Policy check via `check_policy` (ABAC)
2. Channel lookup from `ChannelManager` (DashMap cache + Store persistence)
3. Verify channel enabled
4. Resolve adapter by `ChannelType`
5. `execute_delivery`: decrypt recipient → resolve message body (decrypt if `body_encrypted`) → call adapter → zeroize recipient
6. Emit audit event to Chronicle
7. Return `DeliveryReceipt`

**2. CHANNEL CREATE flow** (engine.rs:112):
1. Policy check
2. `channel_manager.create` — validate name, insert into DashMap cache, check for duplicates
3. `channel_manager.save` — persist to Store as JSON under `courier.channels` namespace
4. Emit audit event
5. Log creation

**3. NOTIFY_EVENT flow** (engine.rs:180):
1. Look up channel
2. Extract `default_recipient` (error if absent)
3. Construct `DeliveryRequest` with plaintext body (no encryption for internal engine events)
4. Delegate to `self.deliver()`

### Capability gating

No compile-time feature flags. All capability gating is runtime via optional trait objects:
- `Decryptor`: `Option<Arc<dyn Decryptor>>` — if None, treats ciphertexts as plaintext with warning
- `PolicyEvaluator`: `Option<Arc<dyn PolicyEvaluator>>` — if None, all policies pass (allow-by-default when no evaluator configured)
- `ChronicleOps`: `Option<Arc<dyn ChronicleOps>>` — if None, audit is a no-op

---

## Cryptographic Constructs

Courier itself implements **no cryptographic primitives**. All cryptography is delegated:

- **Decryption:** Via `shroudb-cipher-client` connecting to a remote Cipher server over TCP. Cipher handles the actual decryption (algorithm, key selection via keyring). Courier receives base64-encoded plaintext, decodes via `base64::engine::general_purpose::STANDARD`.
- **Per-request connections:** `CipherDecryptor` creates a fresh TCP connection per decrypt call — avoids serialization, enables concurrent decryption (~2-5ms overhead measured acceptable vs. connection pooling risks).
- **Master key:** Sourced via `SHROUDB_MASTER_KEY` (hex) or `SHROUDB_MASTER_KEY_FILE` environment variables. Passed to `shroudb_server_bootstrap::open_storage` for at-rest encryption of the embedded Store. Ephemeral key generated in dev mode if neither is set.
- **Zeroization:** `zeroize` crate (derive feature). `RenderedMessage` implements `Drop` to zeroize `body` and `subject`. `execute_delivery` explicitly zeroizes `plaintext_recipient` and `plaintext_body` after adapter call returns.
- **TLS:** SMTP uses `tokio1-rustls-tls` (lettre). HTTP uses `rustls-tls` (reqwest). No OpenSSL dependency.
- **Core dump prevention:** Handled by `shroudb_server_bootstrap` (not visible in this repo).

---

## Engine Relationships

### Calls out to
- **Cipher** (via `shroudb-cipher-client`) — decrypt recipients and message bodies at delivery time
- **Store** (via `shroudb-store` / `shroudb-storage`) — persist channel configurations
- **Chronicle** (via `shroudb-chronicle-core::ChronicleOps`) — audit logging for channel CRUD and deliveries
- **ACL** (via `shroudb-acl`) — token validation, namespace-scoped grants, ABAC policy evaluation

### Called by
- **Moat** — embeds `shroudb-courier-engine` + `shroudb-courier-protocol` for single-binary deployment
- **Other engines** — via `CourierOps::notify()` for operational alerts (Cipher key rotation, Forge cert expiry mentioned in code comments)
- **shroudb-codegen** — reads `protocol.toml` to generate downstream artifacts

### Sentry / ACL integration

**Two-layer authorization:**

1. **Protocol layer** (dispatch.rs): `shroudb_acl::check_dispatch_acl(auth_context, &cmd.acl_requirement())` — validates `AuthContext` against per-command ACL requirements. Public commands (HEALTH, PING, AUTH, COMMAND LIST, HELLO) require no auth. Admin commands (CHANNEL CREATE/DELETE/LIST, DELIVERY GET/LIST, METRICS) require admin. Namespace-scoped commands (CHANNEL GET, DELIVER, NOTIFY_EVENT) require read/write on `courier.{channel}.*`.

2. **Engine layer** (engine.rs): Optional `PolicyEvaluator` (ABAC) for channel_create, channel_delete, deliver. If no evaluator configured, all policies pass — this is the **allow-when-absent** pattern, not the Sentry fallback pattern. The Sentry fallback pattern (deny when Sentry unavailable) is **not implemented** here.

---

## Store Trait

Courier uses `shroudb_store::Store` trait generically (`CourierEngine<S: Store>`). It does not implement Store itself.

- **Storage backend:** `shroudb_storage::EmbeddedStore` is the only wired backend (configured via `[store] mode = "embedded"`). Remote store mode is stubbed but not implemented (`anyhow::bail!("remote store mode not yet implemented")`).
- **Namespace:** Channels persisted under `courier.channels` key prefix.
- **Cache layer:** `DashMap<String, Arc<Channel>>` in `ChannelManager` for O(1) reads. Cache populated from Store on `init()`.

---

## Licensing Tier

**Tier:** Open core (MIT OR Apache-2.0)

All six crates are MIT OR Apache-2.0. No feature flags fence commercial behavior. No license checks in code. No capability traits gate paid-vs-free. The commercial fence is at the **platform level** (Moat integration, Cipher availability, operational tooling) not within Courier itself.

---

## Standalone Extractability

**Extractable as independent product:** With significant work

Courier's value proposition is tightly coupled to Cipher for recipient/body decryption. Without Cipher, it reduces to a simple channel-routing notification dispatcher — commodity functionality trivially replicated by AWS SNS, SendGrid webhooks, or any notification service. The just-in-time decryption of encrypted PII is the differentiator, and that requires Cipher.

To extract standalone:
- Would need to ship Cipher (or a compatible decryption service) alongside
- Would need to replace `shroudb-store`/`shroudb-storage` with a self-contained persistence layer (sqlite, postgres)
- Would need to replace `shroudb-acl` with a standalone auth system
- Would need to replace `shroudb-chronicle-core` with standalone audit logging

### Target persona if standalone
Security-conscious platform teams that store PII encrypted and need notification delivery without exposing plaintext to message queues or logging infrastructure. Compliance-driven orgs (healthcare, finance) where notification recipients are themselves sensitive data.

### Pricing model fit if standalone
Usage-based (per delivery) or infrastructure license. The value scales with delivery volume and compliance requirements, not seats.

---

## Deployment Profile

- **Standalone binary:** `shroudb-courier` TCP server on port 6999, Alpine Docker image (multi-arch amd64/arm64)
- **Embedded library:** `shroudb-courier-engine` + `shroudb-courier-protocol` consumed by Moat
- **CLI tool:** `shroudb-courier-cli` for interactive/batch operations
- **Infrastructure dependencies:** Cipher server (for encrypted deliveries), SMTP server (for email), HTTP endpoints (for webhooks). Storage is embedded (no external DB required).
- **Self-hostable:** Yes, via Docker. Config via TOML file + environment variables. Non-root user (UID 65532). Volume at `/data`.

---

## Monetization Signals

- **Tenant scoping:** Present — ACL grants use `tenant` field on auth tokens, namespace-scoped to `courier.{channel}.*`
- **Quota enforcement:** Absent — no rate limiting, no delivery counters, no usage metering
- **API key validation:** Present — token-based auth with per-token grants
- **Usage tracking:** Absent — audit events go to Chronicle but no aggregation or billing signals
- **License checks:** Absent

---

## Architectural Moat (Component-Level)

The moat is **platform-level, not component-level**. Courier's individual implementation is straightforward — channel CRUD, adapter dispatch, zeroization. What's non-trivial to reproduce is:

1. **The integration with Cipher** — just-in-time decryption at delivery time with per-request connections, concurrent-safe, with guaranteed zeroization. This is a protocol-level design decision, not an algorithm.
2. **The `CourierOps` cross-engine notification pattern** — other engines can fire notifications without knowing delivery details. This is a platform coordination mechanism.
3. **The RESP3 protocol surface** — consistent with all ShrouDB engines, enabling Moat embedding and codegen.

A competitor could reimplement the notification routing in days. The value is in the encrypted-PII-to-delivery pipeline that requires the full ShrouDB stack.

---

## Gaps and Liabilities

1. **No CHANGELOG.md** — version history undocumented despite being at v1.4.2
2. **No LICENSE file** — license specified only in Cargo.toml, not as a file (some registries/tools expect a LICENSE file)
3. **Remote store mode unimplemented** — stubbed with `bail!`, blocking non-embedded deployments
4. **No retry/backoff on delivery failure** — adapter failures return `DeliveryStatus::Failed` receipt immediately. No dead-letter queue, no retry logic. Callers must handle retries.
5. **No delivery persistence** — receipts are returned but not stored. No delivery history, no replay capability.
6. **PolicyEvaluator allow-when-absent** — if no policy evaluator is configured, all operations are allowed. This contradicts the "fail closed" security posture documented in CLAUDE.md. The dispatch-layer ACL is enforced, but the engine-layer ABAC silently permits everything when unconfigured.
7. **No webhook signature verification** — webhook adapter POSTs to URLs without HMAC signing or verification headers. Recipients cannot verify message authenticity.
8. **Workspace version drift** — workspace version is 1.4.2 but internal crate dependency pins reference 1.3.4. This works for path deps but could cause issues if crates are consumed from the registry independently.
9. **Two RUSTSEC advisories suppressed** in deny.toml (PKCS#1 timing side-channel, atomic-polyfill unmaintained) with justifications.

---

## Raw Signals for Evaluator

- **No message templating** — callers render their own messages before submitting. Courier is a dumb pipe with decryption, not a notification platform.
- **NOTIFY_EVENT uses plaintext body** — designed for internal engine alerts, not user-facing encrypted notifications. This means cross-engine notifications bypass Cipher entirely.
- **DashMap for channel cache** — concurrent reads without locking. Good for read-heavy workloads but no cache invalidation beyond restart.
- **Protocol versioning** — `protocol.toml` declares v1.0.0 but no negotiation mechanism exists in the wire protocol.
- **Private registry** — all crates publish to "shroudb" registry, not crates.io. Docker images pushed to Docker Hub via CI.
- **ABOUT.md explicitly states** Courier is not a message queue, not an email service, and not a general notification system. It exists solely to bridge encrypted PII to delivery channels.
- **Test coverage is functional but thin** — ~46 tests covering happy paths and basic error cases. No fuzz testing, no property tests, no load tests. Integration tests spawn the real binary.
- **No metrics endpoint** — HEALTH returns channel count but no Prometheus/OpenTelemetry integration visible.
