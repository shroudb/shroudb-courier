# Understanding ShrouDB Courier

---

## For Everyone: What ShrouDB Courier Does

Applications need to send notifications — welcome emails, password reset links, webhook callbacks, alert messages. The tricky part is that the recipient addresses themselves are often sensitive data. An email address or a webhook URL can reveal who uses a service and how they're connected.

**ShrouDB Courier is a secure notification delivery engine.** It takes encrypted recipient addresses, decrypts them only at the moment of delivery, renders messages from templates, and sends them through the appropriate channel. The recipient's real address is never stored in plaintext — it exists in memory only long enough to send the message, then is wiped clean.

**What it provides:**

- **Multi-channel delivery** — Email (SMTP, SendGrid), webhooks (HTTP POST), and WebSocket (real-time push), with SMS and push notification channels planned.
- **Encrypted recipients** — Recipient addresses are stored as Transit-encrypted ciphertexts. Courier decrypts them at delivery time and immediately wipes the plaintext from memory.
- **Template rendering** — Tera-based templates (Jinja2-like syntax) with variable substitution, supporting subject lines, HTML bodies, and plain text fallbacks.
- **Hot-reloadable templates** — Templates are watched on disk and reloaded automatically when changed, with no server restart required.
- **WebSocket push** — Built-in WebSocket server (default port 7001) for real-time delivery. Clients subscribe to channels and receive messages when Courier delivers via the `ws` channel adapter. Supports E2EE chat flows where Transit encrypts messages and Courier routes ciphertext via WebSocket -- clients decrypt locally.

**Why it matters:**

- Recipient addresses never sit in plaintext in a database, queue, or config file — they're encrypted end-to-end until delivery.
- Plaintext is automatically zeroed from memory after use — even if the process is inspected, the window of exposure is minimal.
- Templates can be updated without downtime — changes are picked up within one second.
- Multiple delivery adapters (SMTP, SendGrid, webhooks) are managed behind a single interface.

---

## For Technical Leaders: Architecture and Trade-offs

### The Problem

Notification systems typically store recipient addresses in plaintext — in databases, message queues, or configuration files. If any of these are breached, every user's contact information is exposed. Meanwhile, managing multiple delivery channels (email providers, webhooks, push services) requires adapter logic that teams rebuild for every project.

### What ShrouDB Courier Is

ShrouDB Courier is a **stateless, secure notification delivery server** built in Rust. It runs as a server on default port 6999, decrypts recipient addresses via a persistent connection to ShrouDB Transit, renders templates, and dispatches messages through pluggable adapters. It stores no persistent state — all configuration lives in memory at runtime.

### Key Architectural Decisions

| Decision | Rationale |
|----------|-----------|
| **Stateless design** | No WAL, no snapshots, no persistent state. Configuration changes are runtime-only. Simplifies deployment and reduces attack surface — there's nothing to steal from disk. |
| **Transit-integrated decryption** | Recipient addresses are Transit ciphertexts. Courier maintains a lazy persistent connection to ShrouDB Transit for decryption. Keys never leave Transit. |
| **Pluggable adapters** | SMTP, SendGrid, webhook, and WebSocket adapters are independently configured. Adding a new channel doesn't require modifying existing ones. |
| **WebSocket server** | Dedicated WebSocket port (7001) with channel-based pub/sub. Enables E2EE chat: Transit encrypts the message, Courier routes ciphertext via WebSocket, and clients decrypt locally. Courier never sees plaintext message content. |
| **Template hot-reload** | File watcher with 1-second debounce picks up template changes without restart. Previous templates remain available if reload fails. |
| **Memory safety by default** | All decrypted plaintext is automatically wiped from memory on drop. Core dumps are disabled on Linux. |

### Delivery Channels

| Channel | Adapter | Status |
|---------|---------|--------|
| Email | SMTP (STARTTLS) | Supported |
| Email | SendGrid API | Supported |
| Webhook | HTTP POST | Supported |
| WebSocket | Channel-based pub/sub | Supported |
| SMS | — | Planned |
| Push | — | Planned |

### Operational Model

- **Configuration:** TOML file with environment variable interpolation. Per-adapter settings for SMTP credentials, SendGrid API keys, and webhook defaults.
- **Observability:** Telemetry via console JSON logs, audit file, and OpenTelemetry export. Audit log for all write operations (deliveries, template reloads).
- **Deployment:** Single static binary. TLS and mTLS supported natively. No external dependencies at runtime beyond ShrouDB Transit.
