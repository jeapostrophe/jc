# Mobile WebSocket Protocol

Protocol specification for communication between the jc desktop app and the iOS companion app.

## Connection

- Transport: WebSocket over TLS (wss://)
- Default port: 9210 (configurable via `config.toml`)
- Certificate: ephemeral self-signed, pinned via SHA-256 fingerprint in QR code

## QR Payload

The QR code encodes a JSON object:

```json
{
  "host": "192.168.1.42",
  "port": 9210,
  "token": "a1b2c3d4e5f6...",
  "fingerprint": "AB:CD:EF:01:23:..."
}
```

| Field         | Description                                  |
|---------------|----------------------------------------------|
| `host`        | LAN IP address of the desktop machine        |
| `port`        | WebSocket server port                        |
| `token`       | Ephemeral auth token (hex, 32 bytes)         |
| `fingerprint` | SHA-256 fingerprint of the TLS certificate   |

## Auth Handshake

1. Client connects via WSS, pinning the certificate fingerprint from QR
2. Server sends `AuthChallenge` with the expected token
3. Client responds with `Auth` containing the token from QR
4. Server sends `AuthResult { success: true }` and begins streaming state
5. On auth failure, server sends `AuthResult { success: false }` and closes

## Message Types

All messages are JSON with a `type` field for discrimination.

### Server -> Client

#### AuthChallenge
```json
{ "type": "AuthChallenge", "token": "a1b2c3d4..." }
```

#### AuthResult
```json
{ "type": "AuthResult", "success": true }
```

#### StateSnapshot
```json
{
  "type": "StateSnapshot",
  "projects": [...],
  "active_project_index": 0,
  "usage": { ... }
}
```

### Client -> Server

#### Auth
```json
{ "type": "Auth", "token": "a1b2c3d4..." }
```

## State Snapshot Schema

```json
{
  "projects": [
    {
      "name": "my-project",
      "sessions": [
        {
          "slug": "feature-auth",
          "label": "Feature: Auth",
          "problems": [
            { "rank": 1, "description": "Permission prompt" }
          ]
        }
      ],
      "active_session_index": 0,
      "problems": [
        { "rank": 10, "description": "Unreviewed: src/main.rs" }
      ]
    }
  ],
  "active_project_index": 0,
  "usage": {
    "par": 12.5,
    "par_status": "Under",
    "limit_pct": 38.0,
    "working_pct": 50.5,
    "five_hour_pct": 22.0,
    "pace": 0.75,
    "remaining_hours": 18.3
  }
}
```

### MobileUsage Fields

| Field             | Type      | Description                              |
|-------------------|-----------|------------------------------------------|
| `par`             | `f64`     | Par differential (positive = under par)  |
| `par_status`      | `string`  | "Under", "Over", or "On"                |
| `limit_pct`       | `f64`     | 7-day budget usage percentage            |
| `working_pct`     | `f64`     | Working time elapsed percentage          |
| `five_hour_pct`   | `f64`     | 5-hour window utilization                |
| `pace`            | `f64?`    | Pace multiplier (< 1.0 = under par)     |
| `remaining_hours` | `f64?`    | Projected hours remaining at current rate|

## State Push Triggers

The server pushes a new `StateSnapshot` after:
- Usage poll completes (every 60s)
- Hook event received (stop, permission, idle)
- Problem list refreshes (every 2s)
- Session switch

## TLS Certificate Pinning

The mobile client validates the server certificate by comparing its SHA-256 fingerprint against the fingerprint embedded in the QR payload. This prevents MITM attacks on the LAN without requiring a CA.

## Configuration

In `~/.config/jc/config.toml`:

```toml
[mobile]
enabled = true
port = 9210
```

Default: `enabled = false`, server does not start.
