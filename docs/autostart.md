# Start clipboard-unifier at login

clipboard-unifier must run inside your desktop session, because that's where the clipboard
lives. The paths below assume you installed with `cargo install`. Adjust them if the binary
is somewhere else.

## macOS (launchd)

Save this as `~/Library/LaunchAgents/dev.clipboard-unifier.plist`, replacing `YOU` with
your username:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>dev.clipboard-unifier</string>
  <key>ProgramArguments</key>
  <array><string>/Users/YOU/.cargo/bin/clipboard-unifier</string><string>run</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>/tmp/clipboard-unifier.log</string>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/dev.clipboard-unifier.plist
```

On macOS 26 the agent may trigger a one-time "Paste from Other Apps" prompt. To allow it
later, go to System Settings → Privacy & Security → Paste from Other Apps.

## Linux (systemd user unit)

Save this as `~/.config/systemd/user/clipboard-unifier.service`:

```ini
[Unit]
Description=clipboard-unifier clipboard sync
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=%h/.cargo/bin/clipboard-unifier run
Restart=on-failure

[Install]
WantedBy=graphical-session.target
```

```bash
systemctl --user import-environment DISPLAY WAYLAND_DISPLAY
systemctl --user enable --now clipboard-unifier
```

## Windows (Task Scheduler, experimental)

This runs it at logon in your session, without a console window:

```bat
schtasks /create /tn clipboard-unifier /sc onlogon /rl limited ^
  /tr "powershell -WindowStyle Hidden -Command \"& '%USERPROFILE%\.cargo\bin\clipboard-unifier.exe' run\""
```
