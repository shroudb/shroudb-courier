# Changelog

All notable changes to ShrouDB Courier are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [v1.4.5] - 2026-04-09

- Version bump release

## [v1.4.4] - 2026-04-09

### Added

- adapt audit events to chronicle-core 1.3.0 resource_type field
- adapt to chronicle-core 1.3.0 event model
- delivery persistence and metrics endpoint (LOW-23, LOW-24)

## [v1.4.2] - 2026-04-04

### Changed

- use shared ServerAuthConfig from shroudb-acl

## [v1.4.1] - 2026-04-02

### Fixed

- use entrypoint script to fix volume mount permissions

### Other

- Use check_dispatch_acl for consistent ACL error formatting

## [v1.4.0] - 2026-04-01

### Other

- Add PolicyEvaluator for ABAC, migrate TCP to shared crate

## [v1.3.6] - 2026-04-01

### Other

- Wire shroudb-server-bootstrap, eliminate startup boilerplate

## [v1.3.5] - 2026-04-01

### Other

- Add plaintext zeroization test for encrypted delivery path

## [v1.3.4] - 2026-04-01

### Other

- Add NOTIFY_EVENT command for rotation/expiry notifications

## [v1.3.3] - 2026-04-01

### Other

- Fail-closed audit for all operations
- Add AGENTS.md

## [v1.3.2] - 2026-03-31

### Other

- Add unit tests to courier-core: channel validation, delivery types (v1.3.2)

## [v1.3.1] - 2026-03-31

### Other

- Arc-wrap channels in cache to avoid cloning on lookup (v1.3.1)

## [v1.3.0] - 2026-03-31

### Other

- Wire ChronicleOps audit events into Courier engine (v1.3.0)

## [v1.2.2] - 2026-03-31

### Other

- Add ACL unit tests to protocol dispatch (v1.2.2)

## [v1.2.1] - 2026-03-31

### Other

- Add concurrency test for delivery (v1.2.1)

## [v1.2.0] - 2026-03-31

### Other

- Zeroize RenderedMessage body on drop (v1.2.0)
- Harden Courier v1.1.0: remove dead config, dedup boilerplate

## [v1.0.0] - 2026-03-30

### Other

- Courier v1: just-in-time decryption delivery engine

