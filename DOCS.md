# ShrouDB Courier Documentation

ShrouDB Courier is a secure notification delivery pipeline. It accepts encrypted recipient addresses, decrypts them at the moment of delivery, renders messages from templates, sends them through the appropriate channel, and wipes the plaintext from memory.

---

## Table of Contents

- [Quick Start](#quick-start)
- [Installation](#installation)
- [Configuration](#configuration)
- [Commands](#commands)
- [Delivery](#delivery)
- [Templates](#templates)
- [Authentication](#authentication)
- [TLS and mTLS](#tls-and-mtls)
- [CLI Client](#cli-client)
- [Rust Client Library](#rust-client-library)
- [Connection URIs](#connection-uris)
- [Observability](#observability)
- [Security](#security)
- [Error Codes](#error-codes)
- [Shutdown Behavior](#shutdown-behavior)

---

## Quick Start

1. Create a configuration file (`courier.toml`):

```toml
[server]
bind = "0.0.0.0:6999"

[transit]
addr = "127.0.0.1:6399"
keyring = "pii"

[templates]
dir = "./templates"
watch = true

[adapters.smtp]
host = "smtp.example.com"
port = 587
username = "user"
password = "${SMTP_PASSWORD}"
from_address = "noreply@example.com"
starttls = true
```

2. Create a template:

```
templates/welcome.subject.txt  ->  Welcome, {{ user_name }}!
templates/welcome.body.html    ->  <h1>Hello {{ user_name }}</h1><p>Welcome to {{ app_name }}.</p>
```

3. Start the server:

```bash
shroudb-courier --config courier.toml
```

4. Send a notification:

```bash
shroudb-courier-cli DELIVER '{"channel":"email","recipient":"<encrypted>","template":"welcome","vars":{"user_name":"Alice","app_name":"Acme"}}'
```

---

## Installation

### From Source

Requires Rust 1.92 or later.

```bash
cargo build --release -p shroudb-courier-server -p shroudb-courier-cli
```

Binaries are written to `target/release/shroudb-courier` and `target/release/shroudb-courier-cli`.

### Docker

The Dockerfile produces two image targets:

| Target | Description |
|--------|-------------|
| `shroudb-courier` | The server. Exposes port 6999. Data volume at `/data`. |
| `shroudb-courier-cli` | The interactive CLI client. |

```bash
docker build --target shroudb-courier -t shroudb-courier .
docker run -p 6999:6999 -v ./config:/data shroudb-courier --config /data/courier.toml
```

Both images run as a non-root user (`shroudb`, UID 65532).

---

## Configuration

Courier is configured via a TOML file (default: `courier.toml`). Environment variables can be interpolated using `${VAR_NAME}` syntax anywhere in the file.

```bash
shroudb-courier --config /path/to/courier.toml
```

If no config file is found, Courier starts with defaults (binds to `0.0.0.0:6999`, no auth, no adapters beyond webhook).

### Server

```toml
[server]
bind = "0.0.0.0:6999"          # Listen address (default: 0.0.0.0:6999)
tls_cert = "/path/to/cert.pem"  # TLS certificate (optional)
tls_key = "/path/to/key.pem"    # TLS private key (optional)
tls_client_ca = "/path/to/ca.pem" # Client CA for mTLS (optional)
rate_limit = 1000                # Max commands/sec per connection (optional)
```

### Transit

Courier connects to a ShrouDB Transit server over TCP to decrypt recipient addresses at delivery time.

```toml
[transit]
addr = "127.0.0.1:6399"   # Transit server address (default: 127.0.0.1:6399)
tls = false                # Use TLS for Transit connection (default: false)
keyring = "pii"            # Transit keyring for decryption (default: "default")
auth_token = "secret"      # Optional auth token for Transit
```

### Templates

```toml
[templates]
dir = "./templates"   # Templates directory (default: ./templates)
watch = true          # Hot-reload on file changes (default: false)
```

### Adapters

#### SMTP

```toml
[adapters.smtp]
host = "smtp.example.com"
port = 587                          # Default: 587
username = "user"                   # Optional (not needed for relay)
password = "${SMTP_PASSWORD}"       # Optional
from_address = "noreply@example.com"
starttls = true                     # Default: true
```

#### SendGrid

```toml
[adapters.sendgrid]
api_key = "${SENDGRID_API_KEY}"
from_email = "noreply@example.com"
from_name = "Acme Service"          # Optional
```

When both SMTP and SendGrid are configured, SendGrid takes precedence for the `email` channel (it registers last).

#### Webhook

The webhook adapter is enabled by default. To disable it:

```toml
[adapters.webhook]
enabled = false
```

For webhooks, the decrypted recipient is the destination URL. Courier sends an HTTP POST with a JSON body containing the rendered message.

---

## Commands

Courier speaks a TCP wire protocol on port 6999 (configurable). All commands follow a verb-argument pattern.

| Command | Description |
|---------|-------------|
| `DELIVER <json>` | Deliver a notification |
| `TEMPLATE_RELOAD` | Reload all templates from disk |
| `TEMPLATE_LIST` | List all loaded template names |
| `TEMPLATE_INFO <name>` | Get metadata for a specific template |
| `HEALTH` | Check server health and adapter status |
| `AUTH <token>` | Authenticate the connection |

---

## Delivery

### Request Format

Send a `DELIVER` command with a JSON payload:

```json
{
  "channel": "email",
  "recipient": "<encrypted-ciphertext>",
  "template": "welcome",
  "vars": {
    "user_name": "Alice",
    "app_name": "Acme"
  }
}
```

**Fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `channel` | Yes | `"email"`, `"webhook"`, `"sms"`, or `"push"` |
| `recipient` | Yes | Encrypted recipient address (Transit ciphertext) |
| `template` | No | Template name to render. Required if `body` is not provided. |
| `vars` | No | Key-value variables for template rendering |
| `subject` | No | Pre-rendered subject line (used when no template is specified) |
| `body` | No | Pre-rendered body (used when no template is specified) |

Either `template` or `body` must be provided. When `template` is set, the template engine renders the subject and body from files. When `body` is provided directly, it is used as-is.

### Delivery Flow

1. Parse and validate the JSON payload
2. Decrypt the recipient via the Transit connection
3. Load and render the template (if specified) with the provided variables
4. Select the adapter for the requested channel
5. Deliver the message via the adapter
6. Wipe the plaintext recipient from memory
7. Return a delivery receipt

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

The `status` field is either `"delivered"` or `"failed"`. On failure, the `error` field contains the reason.

### Supported Channels

| Channel | Adapter | Description |
|---------|---------|-------------|
| `email` | SMTP | Sends via SMTP with STARTTLS support |
| `email` | SendGrid | Sends via the SendGrid API |
| `webhook` | Webhook | HTTP POST to the decrypted recipient URL |
| `sms` | - | Planned |
| `push` | - | Planned |

---

## Templates

Templates use the [Tera](https://keats.github.io/tera/) engine (Jinja2/Django-like syntax). They run in **strict mode** -- missing variables cause an error rather than rendering empty strings.

### File Naming Convention

Place template files in the configured templates directory:

```
{name}.subject.txt    - Subject line (Tera template)
{name}.body.html      - HTML body (preferred for email)
{name}.body.txt       - Plain text body (fallback)
```

A template needs at least one body file (`.body.html` or `.body.txt`). The subject file is optional. When both HTML and text body files exist, the HTML version is used.

### Example

```
templates/
  welcome.subject.txt     "Welcome, {{ user_name }}!"
  welcome.body.html       "<h1>Hello {{ user_name }}</h1><p>Welcome to {{ app_name }}.</p>"
  alert.body.txt          "Alert: {{ message }}"
```

### Template Variables

Variables are passed in the `vars` field of the delivery request. All standard Tera filters and expressions are supported (e.g., `{{ name | upper }}`, `{% if condition %}...{% endif %}`).

### Hot Reload

When `watch = true` in configuration, Courier monitors the templates directory for changes. Modified, added, or removed template files are picked up automatically with a 1-second debounce. No server restart is required.

Templates can also be reloaded manually with the `TEMPLATE_RELOAD` command.

### Inspecting Templates

- `TEMPLATE_LIST` returns all loaded template names.
- `TEMPLATE_INFO <name>` returns a template's supported channels, variables, and load timestamp.

---

## Authentication

Courier supports two authentication modes:

### No Authentication (default)

```toml
[auth]
method = "none"
```

All commands are permitted on every connection.

### Token-Based Authentication

```toml
[auth]
method = "token"

[auth.policies.app]
token = "app-delivery-token"
commands = ["DELIVER", "TEMPLATE_LIST"]

[auth.policies.admin]
token = "admin-token"
commands = []   # Empty = all commands allowed
```

Each policy maps a bearer token to a list of allowed commands. An empty `commands` list grants access to all commands.

The `AUTH` and `HEALTH` commands are always permitted regardless of policy.

To authenticate a connection:

```
AUTH app-delivery-token
```

---

## TLS and mTLS

### Server TLS

```toml
[server]
tls_cert = "/path/to/cert.pem"
tls_key = "/path/to/key.pem"
```

Both `tls_cert` and `tls_key` must be set together. When configured, all connections require TLS.

### Mutual TLS (mTLS)

Add a client CA certificate to require client certificate authentication:

```toml
[server]
tls_cert = "/path/to/cert.pem"
tls_key = "/path/to/key.pem"
tls_client_ca = "/path/to/client-ca.pem"
```

### Transit TLS

The connection to the Transit server can also be encrypted:

```toml
[transit]
tls = true
```

---

## CLI Client

The `shroudb-courier-cli` binary provides an interactive REPL and one-shot command execution.

### Interactive Mode

```bash
shroudb-courier-cli --host 127.0.0.1 --port 6999
```

Features:
- Tab completion for command names
- Command history (saved to `~/.courier_history`)
- Built-in `help` and `help <command>` for usage details

### One-Shot Mode

Pass the command as trailing arguments:

```bash
shroudb-courier-cli HEALTH
shroudb-courier-cli TEMPLATE_LIST
shroudb-courier-cli DELIVER '{"channel":"email","recipient":"enc:...","template":"welcome","vars":{"user_name":"Alice"}}'
```

### CLI Options

| Flag | Description |
|------|-------------|
| `--uri <uri>` | Connection URI (e.g., `shroudb-courier+tls://token@host:6999`) |
| `--host <host>` | Server host (default: `127.0.0.1`) |
| `-p, --port <port>` | Server port (default: `6999`) |
| `--tls` | Connect with TLS |
| `--json` | Output responses as JSON |
| `--raw` | Output raw wire format |

### Output Modes

- **Human** (default): Formatted, readable output.
- **JSON** (`--json`): Responses serialized as JSON. Useful for scripting.
- **Raw** (`--raw`): Raw wire protocol output.

---

## Rust Client Library

The `shroudb-courier-client` crate provides a typed async client.

### Connecting

```rust
use shroudb_courier_client::CourierClient;

// Plain TCP
let mut client = CourierClient::connect("127.0.0.1:6999").await?;

// TLS
let mut client = CourierClient::connect_tls("prod.example.com:6999").await?;

// From URI (handles TLS and auth automatically)
let mut client = CourierClient::from_uri("shroudb-courier+tls://token@host:6999").await?;
```

### Authenticating

```rust
client.auth("app-delivery-token").await?;
```

### Delivering Notifications

```rust
let result = client.deliver(r#"{
  "channel": "email",
  "recipient": "v3:gcm:encrypted_address...",
  "template": "welcome",
  "vars": {"user_name": "Alice"}
}"#).await?;

println!("Delivered via {}: {}", result.adapter, result.delivery_id);
```

### Managing Templates

```rust
let templates = client.template_list().await?;
let info = client.template_info("welcome").await?;
client.template_reload().await?;
```

### Health Check

```rust
let health = client.health().await?;
```

### Raw Commands

```rust
let response = client.raw_command(&["HEALTH"]).await?;
```

---

## Connection URIs

Courier uses a custom URI scheme for client connections:

```
shroudb-courier://[token@]host[:port]
shroudb-courier+tls://[token@]host[:port]
```

**Examples:**

| URI | Description |
|-----|-------------|
| `shroudb-courier://localhost` | Plain TCP, default port (6999) |
| `shroudb-courier://localhost:7100` | Plain TCP, custom port |
| `shroudb-courier+tls://prod.example.com` | TLS, default port |
| `shroudb-courier+tls://mytoken@prod.example.com:7100` | TLS with auth token, custom port |

---

## Observability

Courier uses structured logging via `shroudb-telemetry`. All telemetry is emitted as structured events -- there is no HTTP metrics endpoint.

### Audit Events

Write operations (`DELIVER`, `TEMPLATE_RELOAD`) are logged at INFO level:

| Field | Description |
|-------|-------------|
| `op` | Command verb |
| `result` | `ok` or `error` |
| `duration_ms` | Execution time in milliseconds |
| `actor` | Authenticated policy name or `"anonymous"` |

### Operational Events

| Event | Level | When |
|-------|-------|------|
| Templates loaded | INFO | Startup, reload |
| Adapter registered | INFO | Startup (per adapter) |
| Transit configured | INFO | Startup |
| Template hot-reload | INFO/ERROR | File change detected |
| Connection lifecycle | DEBUG/WARN | Connect, disconnect, errors |
| Server lifecycle | INFO | Startup, shutdown |

### OpenTelemetry

Courier supports OpenTelemetry (OTEL) export for traces and metrics. Configure via environment variables as per the OpenTelemetry SDK specification (e.g., `OTEL_EXPORTER_OTLP_ENDPOINT`).

---

## Security

### Recipient Protection

- **Encrypted at rest:** Recipient addresses are stored as encrypted ciphertexts. Courier never persists them.
- **Decrypted in memory only:** Plaintext exists in memory only for the duration of the adapter call.
- **Zeroized on drop:** All decrypted plaintext is wrapped in `SecretBytes`, which overwrites memory with zeros when the value goes out of scope.
- **Core dumps disabled:** On Linux, core dumps are disabled at startup to prevent leaking decrypted data.

### Template Safety

- **No code execution:** The template engine supports variable interpolation and filters only -- not arbitrary code.
- **Strict mode:** Missing variables produce an error rather than empty strings, preventing silent data leakage.
- **HTML escaping:** Available via the built-in `escape` filter for HTML body templates.

### Network Security

- **TLS:** Server-side TLS encrypts all client connections.
- **mTLS:** Mutual TLS verifies client identity via certificate authentication.
- **Transit TLS:** The connection to the Transit decryption server can be independently encrypted.

### What Courier Does NOT Protect Against

- A compromised Transit server (decryption keys would be exposed).
- A compromised delivery adapter (once handed to SMTP or SendGrid, Courier has no control).
- Recipient inference from delivery patterns (timing, frequency).
- Template name injection (template names should be application-controlled, not user-supplied).

---

## Error Codes

| Code | Description |
|------|-------------|
| `DENIED` | Authentication required or insufficient permissions |
| `NOTFOUND` | Template not found |
| `BADARG` | Missing or invalid argument |
| `DELIVERY_FAILED` | Adapter delivery failed |
| `TEMPLATE_ERROR` | Template rendering error |
| `NOTREADY` | Server is starting up or shutting down |
| `INTERNAL` | Unexpected server error |

Errors are returned with the error code prefix (e.g., `DENIED Authentication required`).

---

## Shutdown Behavior

On `SIGTERM` or `SIGINT`, Courier performs a graceful shutdown:

1. Stops accepting new connections
2. Drains in-flight connections with a 30-second timeout
3. Aborts remaining connections after the timeout expires
