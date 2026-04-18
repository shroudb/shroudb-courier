# Courier Engine DAG

## Overview

Courier is the just-in-time decryption delivery engine for ShrouDB. It persists
channel configurations (email via SMTP, webhook via HTTP) in the encrypted Store
and decrypts Cipher-encrypted recipients (and optionally message bodies) at the
exact moment of delivery, zeroizing plaintext from memory immediately after the
adapter returns. Callers render their own message content; Courier does not
template. The engine crate is decoupled from transport concerns via `Decryptor`
and `DeliveryAdapter` traits, so it can be embedded in Moat without pulling
SMTP or HTTP client dependencies, while the server binary wires either an
in-process `CipherEngine` decryptor or a TCP-backed Cipher client decryptor,
plus `lettre` (SMTP) and `reqwest` (webhook) adapters.

## Crate dependency DAG

```
                       +----------------------+
                       | shroudb-courier-core |
                       | (Channel, Delivery*, |
                       |  CourierError, ops)  |
                       +----------+-----------+
                                  |
          +-----------------------+-----------------------+
          |                       |                       |
          v                       v                       v
+---------------------+ +-----------------------+ +---------------------+
| shroudb-courier-    | | shroudb-courier-      | | shroudb-courier-    |
| engine              | | protocol              | | client              |
| (CourierEngine,     | | (RESP3 parse/dispatch)| | (Rust SDK over      |
|  ChannelManager,    | |                       | |  shroudb-client-    |
|  Decryptor,         | |                       | |  common)            |
|  DeliveryAdapter,   | |                       | |                     |
|  RetryConfig)       | |                       | |                     |
+----------+----------+ +----------+------------+ +----------+----------+
           |                       |                         |
           +-----------+-----------+                         |
                       |                                     |
                       v                                     v
          +--------------------------+          +---------------------+
          | shroudb-courier-server   |          | shroudb-courier-cli |
          | (TCP binary, SMTP +      |          | (operator CLI)      |
          |  webhook adapters,       |          +---------------------+
          |  CipherDecryptor)        |
          +--------------------------+
```

Internal edges (from each crate's `Cargo.toml`):

- `shroudb-courier-core`: leaf (no internal deps).
- `shroudb-courier-engine` -> `shroudb-courier-core`.
- `shroudb-courier-protocol` -> `shroudb-courier-core`, `shroudb-courier-engine`.
- `shroudb-courier-client` -> (no internal courier crate; domain types re-expressed
  through `shroudb-client-common`).
- `shroudb-courier-server` -> `shroudb-courier-core`, `shroudb-courier-engine`,
  `shroudb-courier-protocol`.
- `shroudb-courier-cli` -> consumes `shroudb-courier-client`.

## Capabilities

- Store-backed channel lifecycle: `CHANNEL CREATE|GET|LIST|DELETE` persisted in
  the `courier.channels` namespace, cached in a `DashMap` on startup.
- Two channel types: `email` (SMTP via `lettre`, STARTTLS-capable) and `webhook`
  (HTTP POST via `reqwest`).
- Just-in-time recipient decryption through the `Decryptor` trait; server wires
  either an `EmbeddedDecryptor` (in-process `CipherEngine` on a `cipher`
  namespace of Courier's own `StorageEngine`) or a `CipherDecryptor` that opens
  a fresh TCP connection to a remote Cipher server per decrypt call.
- Optional decryption of the message body when `body_encrypted` is supplied on
  the delivery request; plaintext recipient and body are `Zeroize`-wiped as
  soon as the adapter returns.
- Configurable exponential-backoff retry (`RetryConfig`: max_retries, base_delay,
  max_delay capped) applied around each adapter call; recipient plaintext is
  decrypted once and reused across retries.
- Event notifications via `NOTIFY_EVENT` / `engine.notify_event()` for
  pre-configured channels that carry a `default_recipient` (intended for engine
  schedulers such as Cipher key rotation or Forge cert expiry).
- Delivery receipts persisted to the `courier.receipts` namespace, retrievable
  by UUID (`DELIVERY GET`) or listed with optional channel filter
  (`DELIVERY LIST`).
- In-memory delivery metrics (total, delivered, failed, per-channel counts)
  exposed via `METRICS`.
- Fail-closed ABAC via `PolicyEvaluator` with explicit `PolicyMode::Open` opt-in;
  absence of an evaluator in `PolicyMode::Closed` denies every operation.
- Chronicle audit emission on `CHANNEL_CREATE`, `CHANNEL_DELETE`, and `DELIVER`
  via the `ChronicleOps` capability. The engine exposes the slot as
  `Capability<Arc<dyn ChronicleOps>>` — `DisabledWithJustification` is a no-op,
  but the server binary now refuses to start without an explicit `[audit]`
  config section (embedded, remote, or disabled-with-justification).
- Per-connection `AUTH <token>` command gated by the `ServerAuthConfig` token
  validator built in `main.rs`.
- Optional HMAC-SHA256 webhook signing: when `webhook_signing_secret` is set,
  each webhook POST includes an `X-ShrouDB-Signature` header.
- Identity handshake (`HELLO`), connectivity (`PING`), command catalogue
  (`COMMAND LIST`), and health (`HEALTH`) commands per `protocol.toml`.

## Engine dependencies

All capability slots on `CourierEngine` use the explicit
`shroudb_server_bootstrap::Capability<T>` enum — `Enabled(...)`,
`DisabledForTests`, or `DisabledWithJustification("<reason>")`. Absence is
never silent; operators must name why they're opting out.

### Dependency: Cipher

Courier server pins `shroudb-cipher-client`, `shroudb-cipher-engine`, and
`shroudb-cipher-core` (all workspace `1.6.0`) and holds a runtime
`Capability<Arc<dyn Decryptor>>`. The engine crate itself does not depend on
Cipher directly; it only knows the `Decryptor` trait. The server supports two
Cipher deployment shapes, selected by `cipher.mode`:

- `embedded` — an in-process `CipherEngine` is constructed on a dedicated
  `cipher` namespace of the same `StorageEngine` Courier uses for its own
  store. Requires `store.mode = "embedded"`. The server idempotently seeds
  the configured keyring (algorithm defaults to `aes-256-gcm`, rotation `90`d,
  drain `30`d) and wires an `EmbeddedDecryptor` that calls the engine directly
  — no network hop. The embedded `CipherEngine` is constructed with its own
  policy/audit slots set to `DisabledWithJustification` because those flow
  through Courier's own Sentry/Chronicle slots.
- `remote` — a `CipherDecryptor` opens a fresh TCP connection to a Cipher
  server per decrypt call. `CipherDecryptor::new` performs a
  connectivity-and-auth probe on startup so misconfigured addresses fail fast.

**What breaks without it.** When `[cipher]` is absent from the server config,
the slot is set to `Capability::DisabledWithJustification("no [cipher] section
in courier config — recipients treated as plaintext")` and the server logs a
warning. `execute_delivery_with_retry` still runs, but `decrypt_value` will
fail on any ciphertext input — the only safe path is for callers to submit
plaintext recipients and plaintext bodies. That is not a production mode for
the Courier product (its reason for existence is encrypted recipients), so in a
deployment without Cipher, Courier effectively degrades to a plain SMTP/webhook
proxy with no PII protection. Courier never stores or accepts plaintext as a
fallback for encrypted fields; it returns `CourierError::DecryptionFailed`.

**What works with it.** Recipients (`DeliveryRequest.recipient`) and optional
message bodies (`DeliveryRequest.body_encrypted`) arrive as Cipher ciphertexts.
At delivery time the engine invokes the configured `Decryptor` (embedded or
remote), uses the plaintext only for the adapter invocation, and `Zeroize`-wipes
the buffer before returning the receipt. Retries reuse the single decrypted
plaintext rather than re-hitting Cipher per attempt.

### Dependency: Chronicle

Courier-engine depends on `shroudb-chronicle-core` (workspace `1.11.0`) for the
`ChronicleOps` trait plus `Engine`, `Event`, and `EventResult` types. There is
no transport dep — the engine takes a `Capability<Arc<dyn ChronicleOps>>` and
calls `record` directly.

**What breaks without it.** When the engine is constructed with an audit slot
resolved to `Capability::DisabledWithJustification(...)`, `emit_audit_event`
is a no-op: `channel_create`, `channel_delete`, and `deliver` still succeed,
but no audit trail is produced. Operational visibility is limited to `tracing`
logs and the in-memory metrics counters in that configuration. The server
binary refuses to start if the `[audit]` section is entirely missing — the
operator must choose `mode = "remote"`, `"embedded"`, or `"disabled"` with a
`justification`, so a Chronicle-less deployment is an explicit, documented
posture rather than a silent default.

**What works with it.** Each channel lifecycle operation and each delivery
emits a Chronicle event keyed on `AuditEngine::Courier` with
`resource_type="channel"` and the channel name as `resource`. If Chronicle is
configured but the `record` call fails, the engine surfaces
`CourierError::Internal("audit failed: ...")` so security-critical callers
can fail closed rather than silently lose audit coverage.

### Dependency: Sentry (policy)

The engine takes a `Capability<Arc<dyn PolicyEvaluator>>` from `shroudb-acl`.
The server binary refuses to start if the `[policy]` section is absent —
operators must pick `mode = "remote"` (Sentry over TCP), `"embedded"`, or
`"disabled"` with a justification. Combined with `policy_mode = "closed"`
(default), this means a misconfigured deployment fails shut rather than
silently allowing all operations.

## Reverse dependencies

- **shroudb-cipher** — pins `shroudb-courier-core` at both the workspace and
  `shroudb-cipher-engine` levels. Cipher consumes only the core domain types
  (no transport or engine logic) so it can construct delivery requests for
  rotation/expiry alerts without taking the full Courier stack as a build
  dependency.
- **shroudb-forge** — pins `shroudb-courier-core` at the workspace level and
  in `shroudb-forge-engine`. Same pattern: Forge emits notifications for
  certificate lifecycle events but does not embed Courier itself.
- **shroudb-moat** — feature-gated `courier` flag pulls
  `shroudb-courier-protocol`, `shroudb-courier-engine`, `shroudb-courier-core`,
  and `reqwest`, and implicitly enables the `cipher` feature so the embedded
  Courier engine can reach an in-process Cipher for decryption. Courier is in
  Moat's default feature list.
- No dependency from Scroll, Sigil, Sentry, Stash, Keep, or Veil
  (`grep` over each sibling repo's `Cargo.toml` found none).

## Deployment modes

**Standalone** (`shroudb-courier-server` binary, default TCP port 6999).
Speaks the RESP3 wire format declared in `protocol.toml`. Loads channels from
the persistent Store on boot, registers the `SmtpAdapter` and `WebhookAdapter`
with the `CourierEngine`, constructs either an `EmbeddedDecryptor` (in-process
`CipherEngine` on the `cipher` namespace of the same `StorageEngine`) or a
`CipherDecryptor` (TCP to a remote Cipher server), and accepts connections via
`shroudb-server-tcp` with bootstrap from `shroudb-server-bootstrap`. URI schemes
`shroudb-courier://` and `shroudb-courier+tls://`.

**Embedded** (inside `shroudb-moat`). Moat enables the `courier` feature, which
pulls `shroudb-courier-engine` and `shroudb-courier-protocol` (but not
`shroudb-courier-server`, so `lettre` and `reqwest` come from Moat's own
dependency graph only when the feature is on). Command dispatch is handled by
`shroudb-courier-protocol` parsing RESP3 frames received over Moat's shared
listener. Because the engine crate exposes delivery behaviour through the
`Decryptor` and `DeliveryAdapter` traits, Moat is free to supply its own
in-process decryptor (a local Cipher engine rather than a TCP client) and
any adapter implementations it chooses.
