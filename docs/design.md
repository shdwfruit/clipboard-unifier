# clipbridge — design

## Intent

Apple's Universal Clipboard syncs copy/paste between a Mac and an iPhone signed into the
same Apple Account. It is built on Handoff (BLE advertisements + peer-to-peer Wi-Fi + keys
provisioned through iCloud), so a Windows or Linux machine cannot join it directly.

clipbridge extends it: the Mac — which is already a Universal Clipboard peer — becomes a
bridge. Copy on the iPhone → the Mac's pasteboard receives it via Universal Clipboard →
clipbridge sends it to Windows/Linux. Copy on Windows/Linux → clipbridge writes it to the
Mac's pasteboard → macOS advertises it to the iPhone like any local copy.

See `universal-clipboard-re.md` for the reverse-engineering findings this is based on.

## Goals

- One binary (`clipbridge`) for macOS, Windows, and Linux (X11 and Wayland).
- Text and images (PNG on the wire).
- Zero-config on a LAN: mDNS discovery, a pairing code shared once.
- End-to-end encrypted, mutually authenticated by the pairing secret.
- iPhone leg works with no iPhone software: it rides on Apple's own Universal Clipboard.

## Non-goals

- Re-implementing Apple's Continuity protocol on Windows/Linux (see RE doc for why).
- Files, rich text/HTML, clipboard history UI, cloud relay. (Possible later.)

## Anti-goals

- Leaking passwords: content marked concealed/transient by password managers is never sent.
- Logging clipboard contents. Logs carry kind and size only.
- Feedback loops / ping-pong between machines.

## Architecture

```
iPhone ──(Apple Universal Clipboard: BLE + AWDL)── Mac ──┐
                                                         ├─ clipbridge mesh (TCP + Noise, LAN)
                                         Windows ────────┤
                                         Linux ──────────┘
```

- **Transport:** TCP (default port 47321). Each connection runs a Noise
  `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s` handshake, PSK = HKDF-SHA256(pairing secret).
  A peer without the secret cannot complete the handshake. App messages are length-prefixed
  and chunked across Noise transport frames (max 65535 bytes each).
- **Discovery:** mDNS-SD service `_clipbridge._tcp.local.` with TXT `g=<group id>` (derived
  from the secret, so it isn't the secret itself), `id=<device id>`, `v=1`. Static peers
  (`--peer host:port`) for networks where multicast is blocked.
- **Engine:** poll the local clipboard (cheap change tokens: `NSPasteboard.changeCount` on
  macOS, `GetClipboardSequenceNumber` on Windows, content hash on Linux). New local content
  → `Clip{id, origin, kind}` → send to all peers. Received clip → dedupe by id → write
  locally → remember its hash so the resulting local change is not re-sent → forward to
  other peers (flood with seen-id dedupe, so non-full-mesh topologies still work).
- **macOS specifics:** detect content that arrived via Universal Clipboard (remote marker
  pasteboard type, see RE doc) for logging; skip `org.nspasteboard.ConcealedType` /
  `TransientType`; `--local-only` writes with `NSPasteboardContentsCurrentHostOnly` so
  bridged content does *not* propagate on to the iPhone.
- **Windows specifics:** skip content carrying `ExcludeClipboardContentFromMonitorProcessing`,
  `CanIncludeInClipboardHistory=0`, or `Clipboard Viewer Ignore`.
