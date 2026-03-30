# ShrouDB Courier — Documentation

## Architecture

Courier is a just-in-time decryption delivery engine. It coordinates between ShrouDB Cipher (for decryption) and external delivery services (SMTP, webhooks) to deliver messages without ever storing plaintext recipients or bodies.

### Delivery Flow

1. Client sends `DELIVER` command with channel, encrypted recipient, and body
2. Courier decrypts recipient via Cipher (TCP connection to Cipher server)
3. If `body_encrypted` is provided, Courier decrypts it via Cipher
4. Courier sends the message via the channel's adapter (SMTP or webhook)
5. All plaintext is zeroized from memory
6. A delivery receipt is returned

### Crate Structure

| Crate | Purpose |
|-------|---------|
| `shroudb-courier-core` | Domain types: Channel, DeliveryRequest, errors |
| `shroudb-courier-engine` | Store-backed logic with channel manager and delivery orchestration |
| `shroudb-courier-protocol` | RESP3 command parsing, ACL mapping, and dispatch |
| `shroudb-courier-server` | TCP server binary with adapter implementations |
| `shroudb-courier-client` | Rust client SDK over TCP/RESP3 |
| `shroudb-courier-cli` | Command-line tool with REPL mode |

## Configuration Reference

### Server

```toml
[server]
tcp_bind = "0.0.0.0:6999"    # TCP listen address (default)
log_level = "info"             # Log level: trace, debug, info, warn, error
```

### Store

```toml
[store]
mode = "embedded"              # "embedded" only for v1
data_dir = "./courier-data"    # Data directory for embedded store
```

### Cipher Integration

```toml
[cipher]
addr = "127.0.0.1:6599"       # Cipher server address
keyring = "pii"                # Keyring name for decryption
auth_token = "optional-token"  # Optional auth token for Cipher
```

When `[cipher]` is omitted, Courier operates in plaintext mode (development only). Recipients and encrypted bodies are passed through without decryption.

### Authentication

```toml
[auth]
method = "token"

[auth.tokens.my-token]
tenant = "tenant-a"
actor = "my-app"
platform = false
grants = [
    { namespace = "courier.email-prod.*", scopes = ["read", "write"] },
]
```

### Channel Seeding

Channels can be pre-created at startup:

```toml
[channels.email-prod]
channel_type = "email"

[channels.email-prod.smtp]
host = "smtp.example.com"
port = 587
from_address = "noreply@example.com"
starttls = true
username = "user"
password = "pass"

[channels.events]
channel_type = "webhook"

[channels.events.webhook]
timeout_secs = 30
```

## Command Reference

### CHANNEL CREATE

Create a delivery channel.

```
CHANNEL CREATE <name> <type> <config_json>
```

- `name`: Alphanumeric with hyphens/underscores, max 255 chars
- `type`: `email` or `webhook`
- `config_json`: SMTP config for email, webhook config for webhook

**Email config:**
```json
{"host":"smtp.example.com","port":587,"from_address":"noreply@example.com","starttls":true}
```

**Webhook config:**
```json
{"timeout_secs":30}
```

### CHANNEL GET

```
CHANNEL GET <name>
```

Returns the full channel configuration including adapter settings.

### CHANNEL LIST

```
CHANNEL LIST
```

Returns count and list of channel names.

### CHANNEL DELETE

```
CHANNEL DELETE <name>
```

### DELIVER

```
DELIVER <json>
```

Delivery request format:

```json
{
    "channel": "email-prod",
    "recipient": "<cipher-encrypted-email>",
    "subject": "Welcome",
    "body": "Direct plaintext body",
    "body_encrypted": "<cipher-encrypted-body>",
    "content_type": "html"
}
```

**Required fields:** `channel`, `recipient`, and one of `body` or `body_encrypted`.

**Priority:** `body_encrypted` > `body`. Callers are responsible for rendering their own message content.

### AUTH / HEALTH / PING / COMMAND LIST

Standard operational commands following the ShrouDB engine pattern.

## ACL Model

| Command | Requirement |
|---------|-------------|
| AUTH, HEALTH, PING, COMMAND LIST | None |
| CHANNEL LIST | None |
| CHANNEL CREATE/DELETE | Admin |
| CHANNEL GET | Namespace Read (`courier.{name}.*`) |
| DELIVER | Namespace Write (`courier.{channel}.*`) |

## CLI Usage

### Single Command

```bash
shroudb-courier-cli CHANNEL LIST
shroudb-courier-cli --addr 10.0.0.1:6999 HEALTH
```

### Interactive Mode

```bash
shroudb-courier-cli
courier> AUTH my-token
OK
courier> CHANNEL LIST
{
  "status": "ok",
  "count": 2,
  "channels": ["email-prod", "events"]
}
courier> quit
```

## Rust Client SDK

```rust
use shroudb_courier_client::CourierClient;

let mut client = CourierClient::connect("127.0.0.1:6999").await?;
client.auth("my-token").await?;

// Create channel
client.channel_create("events", "webhook", "{}").await?;

// Deliver
let receipt = client.deliver(r#"{"channel":"events","recipient":"<encrypted>","subject":"Alert","body":"test message"}"#).await?;
```

## Error Codes

| Code | HTTP | Description |
|------|------|-------------|
| NOTFOUND | 404 | Channel not found |
| EXISTS | 409 | Channel already exists |
| BADARG | 400 | Invalid argument or malformed JSON |
| DENIED | 403 | Access denied by ACL |
| AUTH_REQUIRED | 401 | Authentication required |
| ADAPTER | 502 | Delivery adapter failure |
| DECRYPT | 500 | Cipher decryption failed |
| INTERNAL | 500 | Unexpected internal error |
