# Linn Selekt DSM Integration

The Selekt DSM should be controlled through the standard Linn DS/DSM control surfaces rather than a Selekt-specific API.

Primary references checked on 2026-05-09:

- Linn custom API overview: https://docs.linn.co.uk/wiki/index.php/Custom%3AAPI
- Linn LPEC protocol: https://docs.linn.co.uk/wiki/index.php/Developer%3ALPEC
- Selekt DSM /1 Edition Hub page: https://docs.linn.co.uk/wiki/index.php/Selekt_DSM_/1_Edition_Hub

## Recommended First Protocol

Use LPEC first. It is a TCP line protocol on port 23, and Linn documents it as a telnet-like way to access the same UPnP services used by DS/DSM products. This is much smaller than embedding a UPnP control point and fits the ESP32 path better.

The new `linn-lpec` crate keeps the LPEC command format, message parser, and synchronous client wrapper `no_std`. Firmware can provide a TCP transport by implementing `linn_lpec::Transport`.

Useful first actions:

- Playback: `MediaRenderer/AVTransport` version 1 actions `Play`, `Pause`, `Stop`, `Next`, `Previous`.
- Volume/mute: `Preamp/Preamp` version 1 actions `Volume`, `SetVolume`, `Mute`, `SetMute`.
- Source selection: `Ds/Product` version 2 actions `SourceCount`, `Source`, `SetSourceBySystemName`.
- Pins: start with `Ds/Pins` action `InvokeId`; verify exact action support against the device service XML.

## Hardware Bring-Up Checklist

1. Get the Selekt IP address from the Linn front panel/service menu or the router.
2. Confirm LPEC is reachable: `nc -vz <selekt-ip> 23`.
3. Open `http://<selekt-ip>:55178/Ds/device.xml` or the equivalent port advertised by UPnP discovery, then inspect service XML for `Ds/Product`, `Preamp/Preamp`, and `Ds/Pins`.
4. If the embedded CI Gateway is enabled, its docs should be available at `http://<selekt-ip>:4100/apidoc`; keep it as a later option for richer browsing/streaming-service control.
5. Capture real `ALIVE` lines and service action names from the device before wiring UI events to commands.
