## LPEC persistent connection and event subscriptions

Status: open

Replace the current short-lived, polling-heavy LPEC control path with one persistent LPEC session that subscribes to evented services, maintains cached hi-fi status, and serializes commands through a single request/response lane.

### Context

The current `app-runtime::lpec` implementation opens a new TCP connection for each command or status read. Status polling then sends several LPEC `ACTION` messages sequentially every few seconds while the hi-fi screen is active. This works, but it prevents using LPEC subscriptions and adds unnecessary connection churn.

LPEC supports `SUBSCRIBE` and unsolicited `EVENT` messages. Those events can provide initial state and later changed variables for subscribed services. LPEC responses do not include request ids, so command pipelining should not be used as the primary optimization; keep commands serialized unless device testing proves pipelining is safe.

### Proposed Design

- Keep one persistent LPEC connection while hi-fi control is active, or while the runtime needs Linn state.
- Perform startup sync and the blank-line workaround on connection establishment.
- Subscribe to the relevant services, likely starting with `Ds/Playlist`, `Ds/Time`, `Ds/Info`, `Preamp/Preamp`, `Ds/Volume`, and any service needed for source or standby state.
- Add a single socket owner that reads all incoming lines and routes messages by type:
  - `EVENT` updates cached `HifiStatus`.
  - `ALIVE` and `BYEBYE` update connection/service availability.
  - command responses complete the currently in-flight command.
  - unsolicited unsubscribe or errors trigger resubscription or reconnect as appropriate.
- Queue user commands and send only one command at a time.
- Let the UI/runtime read cached `HifiStatus` instead of polling the device for every refresh interval.
- Keep a slow fallback poll only for fields that are not evented or prove unreliable on the Selekt DSM.

### Acceptance Criteria

- `linn-lpec` exposes client operations for `SUBSCRIBE`, `UNSUBSCRIBE`, and reading raw parsed messages without forcing the synchronous `action()` loop.
- `app-runtime::lpec` can maintain a persistent session and cached status from LPEC events.
- User commands remain ordered and have at most one in-flight LPEC request per connection.
- Status updates no longer require opening a new TCP connection every poll interval.
- Reconnect behavior handles closed connections, device reboot, `BYEBYE`, and revoked subscriptions.
- Unit tests cover subscription parsing/routing, event-to-status updates, command serialization, and reconnect/resubscribe decisions.
- Host checks still pass with `cargo fmt --check`, `cargo check`, and `cargo test -p app-core`.

### Notes

- Do not put platform socket ownership into `app-core`; keep shared application behavior deterministic and `no_std`.
- Avoid relying on LPEC command pipelining unless it is tested against the target device and documented.
- CI Gateway may still be the better long-term path for richer metadata, especially Qobuz-specific state.
