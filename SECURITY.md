# Security policy

clipboard-unifier moves clipboard contents between machines, so security reports matter.

## Reporting a vulnerability

Please report vulnerabilities privately, using GitHub's
[**Report a vulnerability**](https://github.com/shdwfruit/clipboard-unifier/security/advisories/new)
form. Don't open a public issue.

Include the affected version or commit, your platform, and steps to reproduce. You should
get a reply within a week.

## Scope

In scope:

- the Noise handshake and transport (`src/transport.rs`, `src/crypto.rs`)
- pairing-code handling and config file permissions
- leaking clipboard contents through logs or discovery
- syncing content that a password manager marked as concealed or transient

Out of scope: Apple's Universal Clipboard itself. Report issues there to
[Apple](https://security.apple.com/).
