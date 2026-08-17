# Linn Selekt DSM Integration

The Selekt DSM should be controlled through the standard Linn DS/DSM control surfaces rather than a Selekt-specific API.

Primary references checked on 2026-05-09:

- Linn custom API overview: https://docs.linn.co.uk/wiki/index.php/Custom%3AAPI
- Linn LPEC protocol: https://docs.linn.co.uk/wiki/index.php/Developer%3ALPEC
- Selekt DSM /1 Edition Hub page: https://docs.linn.co.uk/wiki/index.php/Selekt_DSM_/1_Edition_Hub

## Control Surfaces

Prefer CI Gateway for the Selekt DSM if it is enabled and its device-hosted API documentation is reachable. Linn describes CI Gateway as its Custom Installation API for Crestron and Control4 use. It discovers Linn DS/DSM devices on the local network and exposes JSON messages over WebSocket. For CI Gateway running on a DSM, the configuration page is exposed over HTTPS on port 4100, while programmatic WebSocket traffic uses port 8088. For CI Gateway running through Kazoo Server, WebSocket traffic uses port 4100.

The first real use case is Qobuz playback status and track metadata. That makes CI Gateway the primary integration target, because Linn documents CI Gateway as supporting third-party streaming-service access and configuration, including Qobuz account login. LPEC may still expose the currently playing transport/service state, but it is a poor first choice for Qobuz-specific metadata because it only maps lower-level device services and requires service/action discovery from the DSM.

Treat LPEC as the low-level fallback protocol. LPEC is not an industry standard; it is Linn's custom Linn Products Event Control protocol. It is still useful because Linn documents it as a small TCP line protocol on port 23 that maps onto the same UPnP services used by DS/DSM products.

The `linn-lpec` crate keeps the LPEC command format and message parser `no_std`. `app-runtime` wraps that protocol as an async session-backed `HifiController`, plus shared HTTP/JPEG artwork loading through Zune. The simulator provides an async host TCP connector for development; firmware uses its `embassy-net` sockets directly in the constrained runtime loop while still reusing the shared LPEC and artwork code.

The new `linn-ci-gateway` crate contains the CI Gateway WebSocket request paths and JSON request builders from the DSM-hosted Swagger schema. The page at `http://192.168.7.218:4100/res/api.html?socket=ws%3A%2F%2F192.168.7.218%3A8088%2Fws#!/API_V2/post_V2_transport_play` loads `/api/swagger.yaml` through the WebSocket-backed Swagger UI. The relevant V2 request envelope is a JSON object with `requestPath`, `session`, `room`, optional `tag`, and optional `update`.

Useful first actions:

- CI Gateway: create a session, identify the room/player for the Selekt DSM, subscribe to playback state, and inspect the API V2 endpoint that reports the active Qobuz item.
- LPEC fallback: playback commands through `MediaRenderer/AVTransport`, volume/mute through `Preamp/Preamp`, and source selection through `Ds/Product`.

## Hardware Bring-Up Checklist

1. Get the Selekt IP address from the Linn front panel/service menu or the router.
2. Confirm LPEC is reachable: `nc -vz <selekt-ip> 23`.
3. Enable CI Gateway in Manage Systems if it is not already enabled, then reboot the DSM if prompted.
4. Open `https://<selekt-ip>:4100` and use the API Documentation link in the gateway options.
5. Confirm Qobuz is logged in through the CI Gateway configuration page.
6. Connect test tooling to the DSM-hosted WebSocket endpoint on port 8088 and capture the session creation, room/player discovery, playback-state subscription, and current-track metadata response while Qobuz is playing.
7. Open `http://<selekt-ip>:55178/Ds/device.xml` or the equivalent port advertised by UPnP discovery only if we need LPEC fallback details.
