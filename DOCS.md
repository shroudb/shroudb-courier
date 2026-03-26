# ShrouDB Courier Documentation

---

## Installation

### Homebrew

```bash
brew install shroudb/tap/shroudb-courier
```

This installs both `shroudb-courier` (server) and `shroudb-courier-cli` (interactive CLI).

### Docker

```bash
# Server
docker pull ghcr.io/shroudb/shroudb-courier:latest

# CLI
docker pull ghcr.io/shroudb/shroudb-courier-cli:latest
```

### Binary

Download a prebuilt static binary from the [GitHub Releases](https://github.com/shroudb/shroudb-courier/releases) page. Binaries are available for Linux (x86_64, aarch64) and macOS (Apple Silicon, Intel).

---

## Quick Start

1. **Start the server** with a configuration file:

```bash
shroudb-courier --config courier.toml
```

2. **Connect with the CLI:**

```bash
shroudb-courier-cli --addr 127.0.0.1:6999
```

3. **Authenticate** (if auth is enabled):

```
> AUTH my-token
OK
```

4. **Send a notification:**

```
> DELIVER {"channel": "email", "recipient": "v3:gcm:...", "template": "welcome", "vars": {"user_name": "Alice"}}
```

5. **Check server health:**

```
> HEALTH
READY
```

---

## Configuration

ShrouDB Courier is configured via a TOML file. Environment variables can be interpolated using `${VAR_NAME}` syntax.

### Full Configuration Reference

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
addr = "127.0.0.1:6499"               # ShrouDB Transit server address
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

### Transit Connection

Courier requires a running ShrouDB Transit instance for decrypting recipient addresses. Configure the `[transit]` section with the Transit server address and the keyring used to encrypt your recipient data. Keys never leave Transit — Courier sends ciphertexts to Transit and receives plaintext back over the connection.

### Adapters

Adapters are configured independently under `[adapters.<name>]`. Only configured adapters are available at runtime. You can enable multiple email adapters simultaneously; Courier selects the appropriate one based on the delivery request.

### Templates

Set `dir` to the directory containing your template files. When `watch = true`, Courier monitors the directory for changes and reloads templates automatically with a 1-second debounce. If a reload fails, the previous templates remain active.

---

## Commands Reference

### DELIVER

Deliver a notification. Decrypts the recipient via Transit, renders the template, and sends via the appropriate adapter.

```
DELIVER <json_payload>
```

Returns a delivery receipt with `delivery_id`, `channel`, `adapter`, `status`, and `delivered_at`.

### TEMPLATE_RELOAD

Force-reload all templates from disk.

```
TEMPLATE_RELOAD
```

Returns the count of templates loaded.

### TEMPLATE_LIST

List all currently loaded templates.

```
TEMPLATE_LIST
```

Returns an array of template names.

### TEMPLATE_INFO

Get metadata about a specific template.

```
TEMPLATE_INFO <name>
```

Returns the template name, supported channels, variables, and load timestamp.

### HEALTH

Check server health and adapter status.

```
HEALTH
```

Returns the server state (READY, STARTING, etc.).

### AUTH

Authenticate the current connection.

```
AUTH <token>
```

Returns OK on success. `AUTH` and `HEALTH` are always permitted regardless of policy configuration.

### CHANNEL_INFO

Get the subscriber count for a WebSocket channel.

```
CHANNEL_INFO <channel>
```

Returns the channel name and number of active subscribers. Returns an error if WebSocket is not enabled.

### CHANNEL_LIST

List all active WebSocket channels with their subscriber counts.

```
CHANNEL_LIST
```

Returns an array of channel names and subscriber counts. Returns an error if WebSocket is not enabled.

### CONNECTIONS

Get the total number of active WebSocket connections across all channels.

```
CONNECTIONS
```

Returns the total connection count. Returns an error if WebSocket is not enabled.

### CONFIG

View or modify runtime configuration. Changes are in-memory only (Courier is stateless).

```
CONFIG GET <key>
CONFIG SET <key> <value>
CONFIG LIST
```

Mutable keys: `transit.addr`, `transit.keyring`, `templates_dir`.

### PIPELINE

Batch multiple commands in a single request.

```
PIPELINE <cmd1> END <cmd2> END ...
```

---

## Delivery Request Format

The `DELIVER` command accepts a JSON payload with the following fields:

```json
{
  "channel": "email",
  "recipient": "<Transit-encrypted ciphertext>",
  "template": "welcome",
  "vars": {"user_name": "Alice", "app_name": "Acme"},
  "subject": "Optional pre-rendered subject",
  "body": "Optional pre-rendered body"
}
```

| Field | Required | Description |
|-------|----------|-------------|
| `channel` | Yes | Delivery channel: `email`, `webhook`, or `ws` |
| `recipient` | Yes | Transit-encrypted recipient address |
| `template` | No | Template name to render subject and body |
| `vars` | No | Key-value pairs for template variable substitution |
| `subject` | No | Pre-rendered subject (bypasses template for subject) |
| `body` | No | Pre-rendered body (bypasses template entirely) |

If `template` is provided, the template engine renders the subject and body from files. If `body` is provided directly, it is used as-is with no template lookup.

### Delivery Receipt

A successful delivery returns:

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

---

## Template File Conventions

Templates are stored as files in the configured templates directory. Each template is identified by name, with file extensions determining the content type:

```
{name}.subject.txt    — Email subject line (Tera template)
{name}.body.html      — HTML body (preferred for email)
{name}.body.txt       — Plain text body (fallback)
```

### Example

For a template named `welcome`:

```
templates/
  welcome.subject.txt
  welcome.body.html
  welcome.body.txt
```

**welcome.subject.txt:**
```
Welcome to {{ app_name }}, {{ user_name }}!
```

**welcome.body.html:**
```html
<h1>Welcome, {{ user_name }}!</h1>
<p>Thanks for joining {{ app_name }}. We're glad to have you.</p>
```

**welcome.body.txt:**
```
Welcome, {{ user_name }}!

Thanks for joining {{ app_name }}. We're glad to have you.
```

### Template Engine

Courier uses the Tera template engine (Jinja2-like syntax). Templates run in **strict mode** — referencing an undefined variable causes an error rather than rendering an empty string. This prevents accidental data leakage through template misconfiguration.

---

## Adapter Configuration

### SMTP

Sends email via an SMTP server with STARTTLS support.

```toml
[adapters.smtp]
host = "smtp.example.com"
port = 587
username = "user"
password = "${SMTP_PASSWORD}"
from_address = "noreply@example.com"
starttls = true
```

### SendGrid

Sends email via the SendGrid API.

```toml
[adapters.sendgrid]
api_key = "${SENDGRID_API_KEY}"
from_email = "noreply@example.com"
from_name = "Acme Service"
```

### Webhook

Delivers notifications as HTTP POST requests to the decrypted recipient URL.

```toml
[adapters.webhook]
enabled = true
```

The webhook adapter POSTs the rendered body to the recipient address (which is the webhook URL, stored as a Transit-encrypted ciphertext like any other recipient).

---

## WebSocket

Courier includes a built-in WebSocket server for real-time push delivery. It runs on its own port (default 7001), separate from the wire protocol and HTTP servers.

### Configuration

```toml
[websocket]
enabled = true                          # default: true
bind = "0.0.0.0:7001"                  # default: 0.0.0.0:7001
max_channels = 10000                    # default: 10000
max_connections_per_channel = 1000      # default: 1000
channel_buffer_size = 256               # broadcast buffer per channel
require_auth = false                    # require auth before subscribe
```

### Client Protocol

Clients connect via WebSocket and send JSON messages. On connection, the server sends a `connected` event with a unique socket ID:

```json
{"event": "connected", "data": {"socket_id": "uuid"}}
```

**Subscribe to a channel:**

```json
{"event": "subscribe", "channel": "room:general"}
```

Server confirms:

```json
{"event": "subscribed", "channel": "room:general", "data": null}
```

**Unsubscribe from a channel:**

```json
{"event": "unsubscribe", "channel": "room:general"}
```

**Application-level ping/pong:**

```json
{"event": "ping"}
```

Server responds:

```json
{"event": "pong", "channel": "", "data": null}
```

**Server-pushed messages** (when DELIVER targets channel `ws`):

```json
{"event": "message", "channel": "room:general", "data": {"body": "hello"}}
```

**Error responses:**

```json
{"event": "error", "channel": "", "data": {"message": "description"}}
```

### Delivering via WebSocket

Use the `ws` channel in a DELIVER command. The `recipient` field is the channel name (not a Transit-encrypted address):

```
DELIVER {"channel": "ws", "recipient": "room:general", "body": "hello everyone"}
```

The message is fan-out delivered to all subscribers of the specified channel. The delivery receipt includes the number of recipients that received the message.

### E2EE Chat Flow

For end-to-end encrypted messaging:

1. The sender encrypts the message payload client-side (e.g., using Transit or client-managed keys).
2. The sender sends `DELIVER {"channel": "ws", "recipient": "room:general", "body": "<ciphertext>"}`.
3. Courier routes the ciphertext to all WebSocket subscribers of `room:general` -- Courier never sees plaintext message content.
4. Receiving clients decrypt the ciphertext locally.

This pattern ensures that Courier acts purely as a routing layer for encrypted payloads.

### Wire Protocol Commands for WebSocket

Three wire protocol commands provide runtime visibility into WebSocket state:

- `CHANNEL_INFO <channel>` -- subscriber count for a specific channel.
- `CHANNEL_LIST` -- all active channels with subscriber counts.
- `CONNECTIONS` -- total active WebSocket connections.

These commands return an error if WebSocket is not enabled in the configuration.

---

## Docker Deployment

### Server

```bash
docker run -d \
  --name courier \
  -p 6999:6999 \
  -v ./courier.toml:/data/courier.toml \
  -v ./templates:/data/templates \
  ghcr.io/shroudb/shroudb-courier:latest \
  --config /data/courier.toml
```

The server image runs as a non-root user (UID 65532) and exposes ports 6999 and 7000. The `/data` directory is the default working directory and is configured as a volume.

### CLI

```bash
docker run --rm -it \
  ghcr.io/shroudb/shroudb-courier-cli:latest \
  --addr host.docker.internal:6999
```

### Building from Source

The included Dockerfile supports multi-architecture builds (amd64 and arm64):

```bash
docker buildx build --target shroudb-courier -t shroudb-courier:local .
```

---

## Telemetry Overview

ShrouDB Courier emits telemetry through three channels:

- **Console logs** — Structured JSON logs to stdout for operational visibility.
- **Audit file** — Write operations (deliveries, template reloads) are logged at INFO level with fields for the command verb, outcome, execution duration, and authenticated actor.
- **OpenTelemetry (OTEL)** — Traces and metrics are exported via the OpenTelemetry protocol for integration with your observability stack (Jaeger, Grafana, Datadog, etc.).

### Key Operational Events

| Event | Description |
|-------|-------------|
| Template load | Templates loaded from disk (count, directory) |
| Adapter registration | Per-adapter type registered (SMTP, webhook, SendGrid) |
| Transit connection | Decryptor setup (address, keyring, TLS) |
| Template hot-reload | Reload success or failure |
| Server lifecycle | Startup, shutdown, graceful drain |

### Shutdown Behavior

On SIGTERM or SIGINT, Courier:

1. Stops accepting new connections
2. Drains in-flight connections with a 30-second timeout
3. Aborts remaining connections after timeout
