# clipbridge

Copy on your iPhone, paste on Windows or Linux — and the other way round.

Apple's Universal Clipboard only works between Apple devices signed into the same Apple
Account. clipbridge uses your Mac as the bridge: the Mac is already a Universal Clipboard
peer, so clipbridge only has to move clips between the Mac and your other computers.

```
iPhone ──(Apple Universal Clipboard)── Mac ──┐
                                             ├── clipbridge (encrypted, local network)
                              Windows PC ────┤
                              Linux box ─────┘
```

- **iPhone → PC:** copy on the iPhone → Universal Clipboard puts it on the Mac's pasteboard →
  clipbridge on the Mac sends it to every other machine.
- **PC → iPhone:** copy on the PC → clipbridge writes it to the Mac's pasteboard → macOS
  offers it to the iPhone exactly like a copy made on the Mac.
- Nothing to install on the iPhone. Windows ↔ Linux ↔ Mac sync works too, with or without
  an iPhone.

Text and images are supported.

## Build

Needs a Rust toolchain (1.88+). The build is pure Rust, with no C dependencies.

```sh
cargo install --path .        # or: cargo build --release → target/release/clipbridge
```

Build on each OS (macOS, Windows, Linux), or cross-compile with your usual Rust setup.

- **Linux:** works on X11 and on Wayland compositors with the data-control protocol (KDE,
  Sway, Hyprland, …). Under GNOME Wayland it uses the XWayland clipboard.
- **Windows:** use the MSVC or GNU toolchain.

## Use

On one machine (the Mac is a good choice):

```sh
clipbridge init            # prints a pairing code: cb1-xxxx-xxxx-…
```

On every other machine:

```sh
clipbridge join cb1-xxxx-xxxx-…
```

Then on all of them:

```sh
clipbridge run
```

Machines on the same network find each other automatically (mDNS). If your network blocks
multicast (some Wi-Fi, VPNs, WSL), point them at each other instead:

```sh
clipbridge run --peer my-mac.local           # or an IP; default port 47321
```

Other commands and flags:

| | |
|---|---|
| `clipbridge doctor` | Check config, clipboard access, and (macOS) Handoff and paste-permission state |
| `clipbridge code` | Print the pairing code again |
| `run --local-only` | macOS: keep clips from other machines on the Mac; don't offer them to the iPhone |
| `run --no-discovery` | Use only `--peer` connections |
| `run --port N` | Listen on a different port |
| `run --max-mb N` | Largest clip to send or accept (default 32 MiB) |
| `RUST_LOG=debug` | Verbose logs (still never includes clipboard contents) |

### iPhone checklist

On the Mac and the iPhone: same Apple Account, Bluetooth and Wi-Fi on, and Handoff enabled
(Mac: System Settings → General → AirDrop & Handoff; iPhone: Settings → General → AirPlay
& Continuity). `clipbridge doctor` on the Mac reports the Handoff setting.

On macOS 26, the first clipboard read may trigger a "Paste from Other Apps" prompt. If
`doctor` reports access as `ask` or `always deny`, allow clipbridge in System Settings →
Privacy & Security → Paste from Other Apps.

## Start automatically

**macOS** — `~/Library/LaunchAgents/dev.clipbridge.plist`, then
`launchctl load ~/Library/LaunchAgents/dev.clipbridge.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>dev.clipbridge</string>
  <key>ProgramArguments</key>
  <array><string>/Users/YOU/.cargo/bin/clipbridge</string><string>run</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>/tmp/clipbridge.log</string>
</dict>
</plist>
```

**Windows** — run at logon in your session, which needs clipboard access, without a
console window:

```bat
schtasks /create /tn clipbridge /sc onlogon /rl limited ^
  /tr "powershell -WindowStyle Hidden -Command \"& '%USERPROFILE%\.cargo\bin\clipbridge.exe' run\""
```

**Linux (systemd user unit)** — `~/.config/systemd/user/clipbridge.service`:

```ini
[Unit]
Description=clipbridge clipboard sync
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=%h/.cargo/bin/clipbridge run
Restart=on-failure

[Install]
WantedBy=graphical-session.target
```

```sh
systemctl --user import-environment DISPLAY WAYLAND_DISPLAY
systemctl --user enable --now clipbridge
```

## Security model

- The pairing code *is* the key. Anyone holding it can read and write your clipboard, so
  treat it like a password. The config file is created with mode `0600` on Unix.
- **Encryption:** every connection runs a Noise `NNpsk0_25519_ChaChaPoly_BLAKE2s`
  handshake. The pre-shared key is HKDF-SHA256 of the pairing secret.
  - A machine without the code can't complete the handshake.
  - Each connection uses fresh ephemeral keys, giving forward secrecy.
  - All traffic is encrypted and authenticated.
- **No cloud:** traffic stays on your network; there is no server.
- **mDNS:** advertisements reveal that a clipbridge group exists. They carry the device
  name, a random device id, and an 8-byte group id derived from the secret, never the
  secret itself.
- **Password managers:** content they mark as private is never sent.
  - macOS: `org.nspasteboard.ConcealedType` or `TransientType`.
  - Windows: `ExcludeClipboardContentFromMonitorProcessing`, `Clipboard Viewer Ignore`, or
    `CanIncludeInClipboardHistory = 0`.
- **Logs:** they show kind and size (`text 42B`), never content.

## Limitations

- **The iPhone leg needs a Mac running clipbridge.** Windows and Linux can't join Universal
  Clipboard directly; see the reverse-engineering notes below for why.
- **About 2 minutes to paste on the iPhone.** Universal Clipboard content expires about
  120 s after the copy, as with Apple's own Mac ↔ iPhone behavior. Copy on the PC, then
  paste on the iPhone within about 2 minutes.
- **iPhone copies are fetched immediately.** On the Mac, Universal Clipboard normally fetches
  data from the iPhone only when you paste. clipbridge reads each new clip right away so it
  can forward it, which starts that transfer at once.
- **Text and images only.** Files, rich text/HTML, and multiple items aren't synced. Images
  travel as PNG.
- **Last copy wins.** No history or merging.
- **Linux password hints:** KDE's `x-kde-passwordManagerHint` isn't detected, because
  arboard can't read arbitrary targets. Linux images are checked about every 1.5 s, and only
  when the clipboard holds no text.
- **Network:** IPv4 only. On first run, the OS firewall may ask whether clipbridge can accept
  incoming connections.
- **Testing:** the Windows backend compiles and passes clippy but has not yet been run on a
  real Windows machine.

## How Universal Clipboard works (reverse engineered)

Short version, from macOS 26.6.2 binaries and logs (full write-up with evidence in
[docs/universal-clipboard-re.md](docs/universal-clipboard-re.md)):

1. A copy goes app → `pboard` → `useractivityd` (the Handoff daemon) over a private XPC
   service that only Apple-entitled processes can use.
2. `useractivityd` advertises the clipboard as a Handoff pseudo-activity over Bluetooth LE.
   The 9-byte payload is a 7-byte hash, an option byte where **bit 0x08 = "clipboard has
   content"**, and a current/time byte. A version bit flips on every new copy. The
   advertisement is encrypted with the device's own Handoff key, which other devices on the
   same Apple Account fetch through IDS or Rapport.
3. On paste, the other device sends the command `pbpaste2` and receives an endpoint for
   the `com.apple.continuitypasteboard` companion service. It then pulls the data over
   AWDL (Bonjour `_continuity._tcp`) and writes it to its pasteboard tagged
   `com.apple.is-remote-clipboard`.
4. The content expires 120 s after the copy.

Windows and Linux can't join natively: the keys require an IDS-registered Apple device, and
the transfer needs AWDL. The Mac is already a legitimate peer, so clipbridge uses it as the
bridge through the one open interface, `NSPasteboard`.

To check that iPhone copies reach your Mac's pasteboard, run
`osascript -l JavaScript scripts/pbwatch.js` and copy something on the iPhone.
To regenerate the raw evidence, run `scripts/re-collect.sh` (read-only).

## Development

```sh
cargo test                     # unit tests + multi-node integration tests (mock clipboards)
scripts/e2e/run.sh             # real Mac pasteboard ↔ real X11 clipboard in a Linux container
```

The end-to-end script needs Docker and builds the Linux peer from
`scripts/e2e/Dockerfile`:

```sh
cargo build --release && docker build -f scripts/e2e/Dockerfile -t clipbridge-e2e .
```

It saves the Mac's text clipboard first and restores it afterwards. Layout:

| Path | Role |
|---|---|
| `src/transport.rs` | Noise channel and framing |
| `src/engine.rs` | Sync decisions: echo suppression, dedupe, size cap |
| `src/node.rs` | Networking: listener, dialers, duplicate-connection handling |
| `src/discovery.rs` | mDNS |
| `src/clipboard/` | Per-OS backends |
