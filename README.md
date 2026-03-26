# ShrouDB Courier

Secure notification delivery engine -- decrypts Transit-encrypted recipients, renders Tera templates, and delivers via SMTP, SendGrid, Webhook, or WebSocket adapters. Plaintext is zeroized from memory after use.

Built on ShrouDB's cryptographic foundation (shroudb-crypto) with Transit integration for recipient decryption.

## Quick Start

```bash
# Start with a configuration file
cargo run -- --config courier.toml
```

Default port: `6999` (TCP), `7001` (WebSocket).

## Installation

### Homebrew

```sh
brew install shroudb/tap/shroudb-courier
```

Installs `shroudb-courier` (server) and `shroudb-courier-cli`.

### Docker

```sh
docker pull ghcr.io/shroudb/shroudb-courier:latest
```

A CLI image is also available:

```sh
docker pull ghcr.io/shroudb/shroudb-courier-cli:latest
```

### Binary

Download prebuilt binaries from [GitHub Releases](https://github.com/shroudb/shroudb-courier/releases). Available for Linux (x86_64, aarch64) and macOS (Apple Silicon, Intel).

---

## Docker

The server image is `ghcr.io/shroudb/shroudb-courier`. It runs as a non-root user on a minimal Alpine base.

**1. Create a config file** (`courier.toml`):

```toml
[server]
bind = "0.0.0.0:6999"

[transit]
addr = "127.0.0.1:6499"
keyring = "pii"

[templates]
dir = "/data/templates"
watch = true

[adapters.smtp]
host = "smtp.example.com"
port = 587
username = "user"
password = "${SMTP_PASSWORD}"
from_address = "noreply@example.com"
starttls = true

[websocket]
enabled = true
bind = "0.0.0.0:7001"
```

**2. Run:**

```bash
docker run -d \
  --name shroudb-courier \
  -p 6999:6999 \
  -p 7001:7001 \
  -v ./courier.toml:/data/courier.toml:ro \
  -v ./templates:/data/templates:ro \
  -e SMTP_PASSWORD="your-smtp-password" \
  ghcr.io/shroudb/shroudb-courier:latest \
  --config /data/courier.toml
```

- Courier is **stateless** -- no WAL, no snapshots, no persistent data directory needed.
- `-v ./templates:/data/templates:ro` -- mounts your template directory read-only.
- `-e SMTP_PASSWORD` -- environment variables are interpolated in the config file.

A CLI image is also available:

```bash
docker run --rm -it ghcr.io/shroudb/shroudb-courier-cli:latest --addr host.docker.internal:6999
```

## Features

- **Multi-channel delivery** -- SMTP, SendGrid, Webhook, and WebSocket adapters, independently configured.
- **Transit-integrated decryption** -- recipient addresses are Transit ciphertexts, decrypted at delivery time. Keys never leave Transit.
- **Tera templates** -- Jinja2-like template engine with strict mode, hot-reloadable from disk with 1-second debounce.
- **WebSocket push** -- real-time delivery via persistent WebSocket connections with channel-based pub/sub.
- **Zeroize discipline** -- all decrypted plaintext is wiped from memory on drop. Core dumps are disabled on Linux.
- **Stateless design** -- no WAL, no snapshots, no persistent state. Simplifies deployment and reduces attack surface.
- **Wire protocol** for programmatic access.
- **Runtime configuration** via CONFIG GET/SET/LIST without restarts.
- **Telemetry** via shroudb-telemetry (console + audit file + OTEL).

## Architecture

```
shroudb-courier/
  shroudb-courier-core/       Core domain types (delivery, templates, adapters, WebSocket registry)
  shroudb-courier-protocol/   Command parsing, dispatch, handlers, auth
  shroudb-courier-server/     TCP server, WebSocket server, config, TLS
  shroudb-courier-client/     Async Rust client library
  shroudb-courier-cli/        Interactive CLI with tab completion
```

## Commands

| Command | Description |
|---------|-------------|
| `DELIVER` | Deliver a notification (decrypt recipient, render template, send) |
| `TEMPLATE_RELOAD` | Force-reload all templates from disk |
| `TEMPLATE_LIST` | List all loaded templates |
| `TEMPLATE_INFO` | Get metadata about a specific template |
| `CHANNEL_INFO` | Get subscriber count for a WebSocket channel |
| `CHANNEL_LIST` | List all active WebSocket channels |
| `CONNECTIONS` | Get total active WebSocket connections |
| `HEALTH` | Server health check |
| `AUTH` | Authenticate the current connection |
| `CONFIG GET/SET/LIST` | Runtime configuration management |

## WebSocket

Courier includes a built-in WebSocket server for real-time push delivery. It runs on its own port (default `7001`), separate from the wire protocol server.

### Protocol

Clients send JSON messages to subscribe to channels and receive pushed notifications:

**Subscribe:**
```json
{"event": "subscribe", "channel": "room:general"}
```

Server responds:
```json
{"event": "subscribed", "channel": "room:general", "data": null}
```

**Unsubscribe:**
```json
{"event": "unsubscribe", "channel": "room:general"}
```

**Ping/Pong:**
```json
{"event": "ping"}
```
```json
{"event": "pong", "channel": "", "data": null}
```

**Server-pushed message** (when a DELIVER targets channel `ws`):
```json
{"event": "message", "channel": "room:general", "data": {"body": "hello"}}
```

**Connection event** (sent on connect):
```json
{"event": "connected", "data": {"socket_id": "uuid"}}
```

### E2EE Chat Flow

For end-to-end encrypted messaging, the flow is:

1. Client encrypts the message payload using Transit (or client-side keys).
2. Client sends `DELIVER {"channel": "ws", "recipient": "room:general", ...}` with ciphertext as the body.
3. Courier routes the ciphertext to all WebSocket subscribers of `room:general` -- Courier never sees plaintext message content.
4. Receiving clients decrypt the ciphertext locally.

### WebSocket Configuration

```toml
[websocket]
enabled = true                          # default: true
bind = "0.0.0.0:7001"                  # default: 0.0.0.0:7001
max_channels = 10000                    # default: 10000
max_connections_per_channel = 1000      # default: 1000
channel_buffer_size = 256               # broadcast buffer per channel
require_auth = false                    # require auth before subscribe
```

## Configuration

```toml
[server]
bind = "0.0.0.0:6999"
# tls_cert = "/path/to/cert.pem"
# tls_key = "/path/to/key.pem"
# tls_client_ca = "/path/to/ca.pem"   # enables mTLS
# rate_limit = 1000                    # commands/sec per connection

[auth]
method = "token"                       # "none" (default) or "token"

[auth.policies.app]
token = "app-delivery-token"
commands = ["DELIVER", "TEMPLATE_LIST"]

[auth.policies.admin]
token = "${COURIER_ADMIN_TOKEN}"
commands = ["*"]

[transit]
addr = "127.0.0.1:6499"               # ShrouDB Transit server address
tls = false                            # use TLS for Transit connection
keyring = "pii"                        # Transit keyring for decryption
# auth_token = "transit-token"         # optional Transit auth token

[templates]
dir = "./templates"                    # templates directory
watch = true                           # hot-reload on file changes

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

[websocket]
enabled = true
bind = "0.0.0.0:7001"
max_channels = 10000
max_connections_per_channel = 1000
channel_buffer_size = 256
require_auth = false
```

Environment variables can be interpolated with `${VAR}` syntax.

## Ports

| Port | Protocol | Purpose |
|------|----------|---------|
| 6999 | TCP | Wire protocol (commands) |
| 7001 | WebSocket | Real-time push delivery |

## Client Library

```rust
use shroudb_courier_client::CourierClient;

let mut client = CourierClient::connect("127.0.0.1:6999").await?;

// Deliver an email notification
client.deliver(r#"{"channel": "email", "recipient": "v3:gcm:...", "template": "welcome", "vars": {"user_name": "Alice"}}"#).await?;

// Deliver via WebSocket
client.deliver(r#"{"channel": "ws", "recipient": "room:general", "body": "hello"}"#).await?;
```

Generated client libraries are available via codegen. See [shroudb-courier-client](https://github.com/shroudb/shroudb-courier/tree/main/shroudb-courier-client).

## What ShrouDB Courier is NOT

- **Not a message queue.** It delivers notifications immediately. There is no retry queue or dead-letter mechanism.
- **Not an email marketing platform.** It sends transactional notifications, not bulk campaigns.
- **Not a template editor.** Templates are files on disk managed outside Courier. Courier loads and renders them.
