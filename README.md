# ShrouDB Courier

Just-in-time decryption delivery engine for encrypted notifications.

## Overview

Courier decrypts Cipher-encrypted recipients and message bodies at the moment of delivery, then immediately zeroizes all plaintext from memory. Sensitive data is never stored in cleartext. Delivery channels are managed through a RESP3 wire protocol.

## Features

- **Just-in-time decryption** — recipients and bodies are decrypted via Cipher only at delivery time
- **Store-backed channels** — email (SMTP) and webhook delivery channels persisted in ShrouDB
- **Encrypted body support** — both recipients and message bodies can be Cipher-encrypted
- **ACL integration** — namespace-scoped access control per channel
- **Plaintext zeroization** — all decrypted data is zeroized immediately after use

## Quick Start

```bash
# Start the server (ephemeral key for development)
shroudb-courier

# Create a webhook channel
shroudb-courier-cli CHANNEL CREATE events webhook '{}'

# Deliver (with plaintext recipient in dev mode)
shroudb-courier-cli DELIVER '{"channel":"events","recipient":"https://example.com/hook","subject":"Alert","body":"hello"}'
```

## Configuration

```toml
[server]
tcp_bind = "0.0.0.0:6999"
log_level = "info"

[store]
mode = "embedded"
data_dir = "./courier-data"

[cipher]
addr = "127.0.0.1:6599"
keyring = "pii"
# auth_token = "optional-cipher-token"

[auth]
method = "token"

[auth.tokens.my-app-token]
tenant = "tenant-a"
actor = "my-app"
platform = false
grants = [
    { namespace = "courier.email-prod.*", scopes = ["read", "write"] },
]

[channels.email-prod]
channel_type = "email"
[channels.email-prod.smtp]
host = "smtp.example.com"
port = 587
from_address = "noreply@example.com"
starttls = true

[channels.events]
channel_type = "webhook"
[channels.events.webhook]
```

## Commands

| Command | Description |
|---------|-------------|
| `CHANNEL CREATE <name> <type> <json>` | Create a delivery channel |
| `CHANNEL GET <name>` | Get channel configuration |
| `CHANNEL LIST` | List all channels |
| `CHANNEL DELETE <name>` | Delete a channel |
| `DELIVER <json>` | Decrypt and deliver a message |
| `HEALTH` | Server health check |
| `PING` | Connectivity check |
| `AUTH <token>` | Authenticate connection |
| `COMMAND LIST` | List available commands |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `SHROUDB_MASTER_KEY` | Master encryption key (hex) |
| `SHROUDB_MASTER_KEY_FILE` | Path to master key file |
| `COURIER_CONFIG` | Config file path |
| `COURIER_DATA_DIR` | Data directory |
| `COURIER_TCP_BIND` | TCP bind address |
| `COURIER_LOG_LEVEL` | Log level |
| `COURIER_ADDR` | CLI: server address |

## Security

- Recipients are Cipher-encrypted — decrypted only at delivery time
- All plaintext is zeroized from memory immediately after use
- Core dumps are disabled to prevent key material leakage
- Fail-closed: missing Cipher configuration rejects encrypted deliveries
- Token-based ACL with namespace-scoped grants per channel

## License

MIT OR Apache-2.0
