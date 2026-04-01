# Courier — Agent Instructions

> Secure notification delivery: routes messages through encrypted channels with just-in-time decryption via Cipher. Recipients and message bodies are decrypted only at delivery time, then zeroized.

## Quick Context

- **Role in ecosystem**: Notification delivery engine — decrypts recipients/bodies via Cipher immediately before sending, never persists plaintext
- **Deployment modes**: embedded | remote (TCP port 6999)
- **Wire protocol**: RESP3
- **Backing store**: ShrouDB Store trait (channel metadata only)

## Workspace Layout

```
shroudb-courier-core/      # Channel, DeliveryRequest, RenderedMessage, DeliveryReceipt, errors
shroudb-courier-engine/    # CourierEngine, ChannelManager, Decryptor trait, DeliveryAdapter trait
shroudb-courier-protocol/  # RESP3 command parsing + dispatch
shroudb-courier-server/    # TCP binary + SmtpAdapter + WebhookAdapter + CipherDecryptor
shroudb-courier-client/    # Typed Rust SDK
shroudb-courier-cli/       # CLI tool
```

## RESP3 Commands

| Command | Args | Returns | Description |
|---------|------|---------|-------------|
| `CHANNEL CREATE` | `<name> <type> <config_json>` | `{status, name, channel_type}` | Create delivery channel (Admin) |
| `CHANNEL GET` | `<name>` | Full channel JSON | Get channel config |
| `CHANNEL LIST` | — | `{status, count, channels}` | List all channels |
| `CHANNEL DELETE` | `<name>` | `{status, name}` | Delete channel (Admin) |
| `DELIVER` | `<request_json>` | `{delivery_id, channel, status, delivered_at, error?}` | Decrypt + deliver notification |
| `AUTH` | `<token>` | `{status}` | Authenticate |
| `HEALTH` | — | `{status, channels}` | Health check with channel count |
| `PING` | — | `PONG` | Liveness |
| `COMMAND LIST` | — | `{commands, count}` | List commands |

### Command Examples

```
> CHANNEL CREATE email-prod email {"host":"smtp.example.com","port":587,"from_address":"noreply@example.com","starttls":true}
{"status":"ok","name":"email-prod","channel_type":"email"}

> DELIVER {"channel":"email-prod","recipient":"cipher:encrypted-email...","body_encrypted":"cipher:encrypted-body...","content_type":"html"}
{"delivery_id":"abc-123","channel":"email-prod","status":"delivered","delivered_at":1711843200}
```

### DELIVER Request Format

```json
{
  "channel": "email-prod",
  "recipient": "cipher:encrypted-recipient",
  "subject": "Optional subject",
  "body": "plaintext body",
  "body_encrypted": "cipher:encrypted-body",
  "content_type": "plain"
}
```

- `channel` + `recipient`: required
- `body` or `body_encrypted`: one required (`body_encrypted` takes priority)
- `content_type`: `"plain"` (default) or `"html"`

## Delivery Backends

### Email (SMTP)

```rust
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_address: String,
    pub starttls: bool,
}
```

Recipient is parsed as email address. Body sent as UTF-8 with content-type header.

### Webhook (HTTP POST)

```rust
pub struct WebhookConfig {
    pub default_method: Option<String>,
    pub default_headers: Option<HashMap<String, String>>,
    pub timeout_secs: Option<u64>,
}
```

Recipient is the webhook URL. Body sent as JSON: `{"subject": "...", "body": "...", "content_type": "..."}`. Success = HTTP 2xx.

## Decryption Flow

```
1. Decrypt recipient via Cipher (ciphertext → plaintext email/URL)
2. Resolve body: body_encrypted → Cipher decrypt, OR use plaintext body
3. Call delivery adapter (SMTP or HTTP)
4. Zeroize plaintext recipient + body from memory
5. Return DeliveryReceipt (contains no plaintext)
```

### Decryptor Trait

```rust
pub trait Decryptor: Send + Sync {
    fn decrypt(&self, ciphertext: &str) -> BoxFut<Result<String, CourierError>>;
}
```

Production: `CipherDecryptor` connects to Cipher server, decrypts via specified keyring.

## Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server.tcp_bind` | `String` | `"0.0.0.0:6999"` | TCP listen address |
| `store.data_dir` | `String` | `"./courier-data"` | Data directory |
| `cipher.addr` | `String` | — | Cipher server address |
| `cipher.keyring` | `String` | — | Keyring for decryption |
| `cipher.auth_token` | `Option<String>` | `None` | Cipher auth token |

### Channel Seeding (config)

```toml
[channels.email-prod]
channel_type = "email"
[channels.email-prod.smtp]
host = "smtp.example.com"
port = 587
from_address = "noreply@example.com"
starttls = true
```

## Data Model

- **Namespace**: `courier.channels`
- **Key**: Channel name (bytes)
- **Value**: JSON-serialized `Channel`
- **Cache**: `DashMap<String, Channel>` — write-through

Courier does **not** store delivery receipts or message content. Only channel configuration is persisted.

## Common Mistakes

- Recipients must be Cipher-encrypted before passing to `DELIVER`. Courier decrypts them JIT — it never receives plaintext recipients.
- If no Cipher decryptor is configured, recipients are treated as plaintext (with a warning). This is for dev only.
- `RenderedMessage.body` implements `Drop` with zeroization — plaintext exists in memory only during adapter execution
- Channel names: alphanumeric + `-` + `_`, 1-255 chars

## Related Crates

| Crate | Relationship |
|-------|-------------|
| `shroudb-store` | Provides Store trait for channel persistence |
| `shroudb-cipher` | Decrypts recipients and message bodies via `Decryptor` trait |
| `shroudb-moat` | Embeds Courier; wires Cipher as the decryptor |
| `lettre` | SMTP delivery adapter |
| `reqwest` | Webhook HTTP delivery adapter |
