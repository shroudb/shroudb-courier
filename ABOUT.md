# Understanding ShrouDB Courier

This document explains ShrouDB Courier at four levels of depth, plus a complete observability reference. Pick the section that matches your background.

---

## For Everyone: What ShrouDB Courier Does

Applications need to send notifications — welcome emails, password reset links, webhook callbacks, alert messages. The tricky part is that the recipient addresses themselves are often sensitive data. An email address or a webhook URL can reveal who uses a service and how they're connected.

**ShrouDB Courier is a secure notification delivery pipeline.** It takes encrypted recipient addresses, decrypts them only at the moment of delivery, renders messages from templates, and sends them through the appropriate channel. The recipient's real address is never stored in plaintext — it exists in memory only long enough to send the message, then is wiped clean.

**What it provides:**

- **Multi-channel delivery** — Email (SMTP, SendGrid), webhooks (HTTP POST), with SMS and push notification channels planned.
- **Encrypted recipients** — Recipient addresses are stored as Transit-encrypted ciphertexts. Courier decrypts them at delivery time and immediately wipes the plaintext from memory.
- **Template rendering** — Tera-based templates (Jinja2-like syntax) with variable substitution, supporting subject lines, HTML bodies, and plain text fallbacks.
- **Hot-reloadable templates** — Templates are watched on disk and reloaded automatically when changed, with no server restart required.

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

ShrouDB Courier is a **stateless, secure notification delivery server** built in Rust. It speaks the RESP3 wire protocol (default port 6999), decrypts recipient addresses via a persistent connection to ShrouDB Transit, renders templates, and dispatches messages through pluggable adapters. It stores no persistent state — all configuration lives in memory at runtime.

### Key Architectural Decisions

| Decision | Rationale |
|----------|-----------|
| **Stateless design** | No WAL, no snapshots, no persistent state. Configuration changes are runtime-only. Simplifies deployment and reduces attack surface — there's nothing to steal from disk. |
| **Transit-integrated decryption** | Recipient addresses are Transit ciphertexts. Courier maintains a lazy persistent connection to Transit for decryption. Keys never leave Transit. |
| **Pluggable adapters** | SMTP, SendGrid, and webhook adapters are independently configured. Adding a new channel doesn't require modifying existing ones. |
| **Template hot-reload** | File watcher with 1-second debounce picks up template changes without restart. Previous templates remain available if reload fails. |
| **Memory safety by default** | All decrypted plaintext is wrapped in `zeroize`-backed types that automatically wipe memory on drop. Core dumps are disabled on Linux. |

### Delivery Channels

| Channel | Adapter | Status |
|---------|---------|--------|
| Email | SMTP (STARTTLS) | Supported |
| Email | SendGrid API | Supported |
| Webhook | HTTP POST | Supported |
| SMS | — | Planned |
| Push | — | Planned |

### Operational Model

- **Configuration:** TOML file with environment variable interpolation. Per-adapter settings for SMTP credentials, SendGrid API keys, and webhook defaults.
- **Observability:** Telemetry via shroudb-telemetry (console JSON, audit file, OpenTelemetry). Audit log for all write operations (deliveries, template reloads).
- **Deployment:** Single static binary. TLS and mTLS supported natively. No external dependencies at runtime beyond the Transit server.

---

## For Backend Engineers: How It Works

### Wire Protocol

ShrouDB Courier speaks RESP3 over TCP (default port 6999). Commands follow a verb pattern:

```
DELIVER <json_payload>                    → delivery receipt
TEMPLATE_RELOAD                           → reloads templates from disk
TEMPLATE_LIST                             → list all loaded templates
TEMPLATE_INFO <name>                      → template metadata
HEALTH                                    → server health + adapter info
AUTH <token>                              → authenticate connection
CONFIG GET|SET|LIST [key] [value]         → runtime configuration
                                            In-memory only (Courier is stateless).
                                            Mutable keys: transit.addr, transit.keyring,
                                            templates_dir.
PIPELINE <cmd1> END <cmd2> END ...        → batch commands
```

### Delivery Flow

```
DELIVER {
  "channel": "email",
  "recipient": "v3:gcm:dGhpcyBpcyBlbmNyeXB0ZWQ=",
  "template": "welcome",
  "vars": {"user_name": "Alice", "app_name": "Acme"}
}

1. Parse JSON payload, validate required fields
2. Decrypt recipient via Transit connection → plaintext email address
3. Load template ("welcome") from in-memory template engine
4. Render subject + body with vars using Tera engine
5. Select adapter (SMTP or SendGrid for email channel)
6. Deliver message via adapter
7. Zeroize plaintext recipient from memory
8. Return delivery receipt with delivery_id, status, adapter used
```

### Delivery Request Format

```json
{
  "channel": "email|webhook",
  "recipient": "<Transit-encrypted ciphertext>",
  "template": "welcome",
  "vars": {"user_name": "Alice"},
  "subject": "Optional pre-rendered subject",
  "body": "Optional pre-rendered body"
}
```

If `template` is provided, the template engine renders subject and body from files. If `body` is provided directly, it's used as-is (no template lookup).

### Delivery Receipt

```json
{
  "delivery_id": "550e8400-e29b-41d4-a716-446655440000",
  "channel": "email",
  "adapter": "smtp",
  "status": "delivered",
  "delivered_at": 1711468800,
  "error": null
}
```

### Template File Convention

Templates are stored as files in the configured templates directory:

```
{name}.subject.txt    — Email subject line (Tera template)
{name}.body.html      — HTML body (preferred for email)
{name}.body.txt       — Plain text body (fallback)
```

The Tera engine runs in strict mode — missing variables cause an error rather than rendering empty strings.

### Configuration

```toml
[server]
bind = "0.0.0.0:6999"
# tls_cert = "/path/to/cert.pem"
# tls_key = "/path/to/key.pem"
# tls_client_ca = "/path/to/ca.pem"   # Enables mTLS
# rate_limit = 1000                    # Commands/sec per connection

[auth]
method = "none"                        # "none" or "token"

[auth.policies.app]
token = "app-delivery-token"
commands = ["DELIVER", "TEMPLATE_LIST"]

[transit]
addr = "127.0.0.1:6499"               # Transit server address
tls = false                            # Use TLS for Transit connection
keyring = "pii"                        # Transit keyring for decryption
# auth_token = "transit-token"         # Optional Transit auth token

[templates]
dir = "./templates"                    # Templates directory
watch = true                           # Hot-reload on file changes

[adapters.smtp]
host = "smtp.example.com"
port = 587
username = "user"
password = "${SMTP_PASSWORD}"
from_address = "noreply@example.com"
starttls = true

[adapters.sendgrid]
api_key = "${SENDGRID_API_KEY}"
from_email = "noreply@example.com"
from_name = "Acme Service"

[adapters.webhook]
enabled = true
```

### Client Library

```rust
let mut client = CourierClient::connect("127.0.0.1:6999").await?;
client.auth("app-delivery-token").await?;

// Deliver a templated email
let result = client.deliver(r#"{
  "channel": "email",
  "recipient": "v3:gcm:encrypted_address...",
  "template": "welcome",
  "vars": {"user_name": "Alice"}
}"#).await?;

println!("Delivered via {}: {}", result.adapter, result.delivery_id);

// List templates
let templates = client.template_list().await?;

// Health check
let health = client.health().await?;
```

URI format: `shroudb-courier://[token@]host[:port]` or `shroudb-courier+tls://[token@]host[:port]`

---

## For Security Engineers: Threat Model and Protections

### Trust Boundaries

```
Untrusted:
  Network traffic (mitigated: TLS/mTLS)
  Delivery request payloads (mitigated: JSON validation, length limits)
  Template variables (mitigated: Tera sandboxing, no code execution)

Trusted:
  The Courier process and its memory space
  The Transit server (provides decryption)
  The delivery adapters (SMTP server, SendGrid API, webhook endpoints)
```

### Recipient Protection

- **Encrypted at rest:** Recipient addresses are stored as Transit ciphertexts. Courier never persists them.
- **Decrypted in memory only:** The Transit connection decrypts recipients at delivery time. Plaintext exists in memory for the duration of the adapter call.
- **Zeroized on drop:** All plaintext strings are wrapped in `SecretBytes` (from `shroudb-crypto`), which implements `Zeroize`. Memory is overwritten with zeros when the value goes out of scope.
- **Core dumps disabled:** On Linux, `prctl(PR_SET_DUMPABLE, 0)` prevents core dumps that could contain decrypted recipients.

### Template Security

- **No code execution:** Tera is a template engine, not a scripting language. Variable interpolation and filters are supported; arbitrary code execution is not.
- **Strict mode:** Undefined variables cause an error rather than rendering as empty strings, preventing accidental data leakage through template misconfiguration.
- **HTML escaping:** Available via Tera's built-in `escape` filter for HTML body templates.

### Authentication and Authorization

- Client connections authenticate with bearer tokens configured in the server's policy file.
- Each token is scoped to specific commands (e.g., a token might only allow `DELIVER` and `TEMPLATE_LIST`).
- `AUTH` and `HEALTH` commands are always permitted regardless of policy.

### What ShrouDB Courier Does NOT Protect Against

- **Compromised Transit server** — If Transit is compromised, decryption keys are exposed and recipients can be decrypted.
- **Compromised delivery adapter** — Once a message is handed to SMTP or SendGrid, Courier has no control over its handling.
- **Recipient inference** — An attacker who can observe delivery patterns (timing, frequency) may infer information about recipients even without decrypting addresses.
- **Template injection** — If user-controlled input is used as template names (not variables), it could reference unintended templates. Template names should be application-controlled.

---

## Observability Reference

ShrouDB Courier uses shroudb-telemetry for all observability: console JSON logs, an audit file, and OpenTelemetry (OTEL) export. There is no `/metrics` endpoint and no HTTP sidecar — Courier is RESP3 only. All telemetry flows through OTEL.

### Audit Log

Write operations are logged at INFO level with target `courier::audit`:

| Field | Description |
|-------|-------------|
| `op` | Command verb (DELIVER, TEMPLATE_RELOAD) |
| `result` | Outcome (ok, error) |
| `duration_ms` | Execution time in milliseconds |
| `actor` | Authenticated policy name or "anonymous" |

### Operational Events

| Event | Level | Description |
|-------|-------|-------------|
| Template load | INFO | Templates loaded from disk (count, directory, watch flag) |
| Adapter registration | INFO | Per-adapter type (SMTP, webhook, SendGrid) |
| Transit connection | INFO | Transit decryptor setup (address, keyring, TLS flag) |
| Template hot-reload | INFO/ERROR | Success (new count) or failure (error details) |
| Connection lifecycle | DEBUG/WARN | EOF (debug), errors (warn) |
| Server lifecycle | INFO | Startup, shutdown, graceful drain |

### Shutdown Behavior

On SIGTERM or SIGINT, Courier:
1. Stops accepting new connections
2. Drains in-flight connections with a 30-second timeout
3. Aborts remaining connections after timeout
