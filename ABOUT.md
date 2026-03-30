# ShrouDB Courier

Courier is a just-in-time decryption delivery engine within the ShrouDB ecosystem. It solves the problem of delivering notifications (email, webhooks) to recipients whose contact information is encrypted at rest — without ever storing plaintext addresses, URLs, or message bodies.

## Why Courier Exists

In a security-first architecture, PII like email addresses and webhook URLs are encrypted using ShrouDB Cipher. But delivery systems need the actual plaintext address to send a message. Courier bridges this gap: it decrypts recipients at the exact moment of delivery, sends the message, and immediately zeroizes all plaintext from memory.

## Design Decisions

**Store-backed channels.** Unlike stateless delivery proxies, Courier persists its channel configurations in ShrouDB's encrypted Store. This means channel setup and adapter configuration survive restarts and can be managed via the standard RESP3 protocol.

**Cipher integration for just-in-time decryption.** Courier connects to a ShrouDB Cipher server over TCP to decrypt recipients and optionally message bodies. This keeps key material centralized in Cipher while Courier handles only the delivery orchestration.

**Adapter-based delivery.** Each channel type (email, webhook) has a dedicated adapter. The engine defines delivery as a trait, allowing the server to provide concrete implementations. This separation means the engine crate can be embedded in Moat without pulling in SMTP or HTTP client dependencies.

**Caller-rendered messages.** Courier does not include a template engine. Callers are responsible for rendering their own message content before submitting a delivery request. This keeps Courier focused on its core responsibility: just-in-time decryption and delivery.

**Plaintext zeroization.** Every code path that touches decrypted data ensures zeroization after use. Recipients, decrypted bodies, and rendered messages are wiped from memory as soon as the delivery adapter returns.

## Ecosystem Position

Courier is a downstream consumer of ShrouDB Cipher (for decryption) and ShrouDB's storage layer (for persistence). It is consumed by Moat for single-binary deployment. Its protocol is described in `protocol.toml` for cross-language client generation via `shroudb-codegen`.

## What Courier Is Not

- **Not a message queue.** Courier delivers immediately. There is no retry queue, dead-letter mechanism, or deferred delivery. If delivery fails, the receipt indicates failure and the caller decides what to do.
- **Not an email service.** Courier routes messages through configured adapters. It does not manage email lists, handle bounces, or track opens.
- **Not a general-purpose notification system.** Courier's value is specifically in just-in-time decryption. If your recipients aren't encrypted, a standard notification service is simpler.
