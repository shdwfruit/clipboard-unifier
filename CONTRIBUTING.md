# Contributing

Thanks for helping. Bug reports, Windows and Wayland test results, and pull requests are
all welcome.

## Reporting a problem

[Open an issue](https://github.com/shdwfruit/clipboard-unifier/issues) and include:

- your OS and version on each machine
- the output of `clipboard-unifier doctor`
- the relevant lines from `RUST_LOG=debug clipboard-unifier run`. Logs never contain
  clipboard contents, only kind and size.

If the iPhone leg misbehaves, also include the output of
`osascript -l JavaScript scripts/pbwatch.js` on the Mac while you copy on the iPhone.

## Development

You need Rust 1.88+.

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test        # unit tests + multi-node integration tests with mock clipboards
```

Check that the other platforms still compile:

```bash
rustup target add x86_64-pc-windows-gnu x86_64-unknown-linux-gnu
cargo check --target x86_64-pc-windows-gnu
cargo check --target x86_64-unknown-linux-gnu
```

### End-to-end test (macOS ↔ Linux)

This runs the real macOS pasteboard against a real X11 clipboard in a Linux container. It
needs Docker (or Colima). The script saves the Mac's text clipboard and restores it at the
end.

```bash
cargo build --release
docker build -f scripts/e2e/Dockerfile -t clipboard-unifier-e2e .
scripts/e2e/run.sh
```

It checks text and images in both directions and fails if a clip is sent more than once.

## Layout

| Path | Role |
|---|---|
| `src/main.rs` | CLI (`init`, `join`, `run`, `code`, `doctor`) |
| `src/node.rs` | Networking: listener, dialers, duplicate-connection handling, relay |
| `src/engine.rs` | Sync decisions: echo suppression, dedupe, size cap |
| `src/transport.rs` | Noise channel and message framing |
| `src/discovery.rs` | mDNS (`_clipunifier._tcp`) |
| `src/clipboard/` | Per-OS backends (`macos.rs`, `windows.rs`, `linux.rs`) and a mock |
| `docs/universal-clipboard-re.md` | The reverse-engineering write-up |
| `scripts/` | `pbwatch.js`, `re-collect.sh`, and the e2e harness |

## Ground rules

- Never log or transmit clipboard contents outside the encrypted channel.
- Keep respecting password-manager markers (`org.nspasteboard.ConcealedType` and its
  equivalents on other platforms).
- Label reverse-engineering claims in the write-up as *seen*, *inferred*, or *prior work*.
