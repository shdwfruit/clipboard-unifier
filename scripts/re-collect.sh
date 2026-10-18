#!/bin/bash
# Regenerates the raw evidence behind docs/universal-clipboard-re.md into research/.
# Read-only: inspects system binaries and logs, never touches the clipboard.
# Output contains Apple binary strings and this Mac's device identifiers, so research/
# is git-ignored.
set -euo pipefail

OUT="$(cd "$(dirname "$0")/.." && pwd)/research"
mkdir -p "$OUT"
UAD=/System/Library/PrivateFrameworks/UserActivity.framework/Agents/useractivityd
OBJDUMP=/Library/Developer/CommandLineTools/usr/bin/objdump
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion))" > "$OUT/host.txt"

# REA evidence for the Handoff daemon (imports: SFActivityAdvertiserOptionFlagCopyPasteKey, ...)
(cd "$TMP" && npx -y rea-agents@latest inspect-macho "$UAD" --format json) > "$OUT/rea-inspect-macho-useractivityd.json"

# ObjC classes and methods (UASharedPasteboard*, UCStreamCoderV2, SharingBTLEAdvertisementPayload)
dyld_info -arch arm64e -objc "$UAD" > "$OUT/useractivityd-objc.txt"

# Strings: useractivityd and sharingd (on disk), CoreFoundation/UserActivity (shared cache)
lipo "$UAD" -thin arm64e -output "$TMP/useractivityd"
lipo /usr/libexec/sharingd -thin arm64e -output "$TMP/sharingd" 2>/dev/null || cp /usr/libexec/sharingd "$TMP/sharingd"
strings -a -n 6 "$TMP/useractivityd" > "$OUT/useractivityd-strings.txt"
strings -a -n 6 "$TMP/sharingd" > "$OUT/sharingd-strings.txt"
dyld_info -all_dyld_cache -section __TEXT __cstring 2>/dev/null \
  | LC_ALL=C awk '/^\/.*\[arm64e\]:$/ {img=$0} /is-remote-clipboard|UASharedPasteboard|com\.apple\.pboard\./ {print img " :: " $0}' \
  > "$OUT/shared-cache-marker-strings.txt"

# Disassembly of the advertisement payload accessors and the "pbpaste!" request
"$OBJDUMP" -d --no-show-raw-insn --start-address=0x10006E288 --stop-address=0x10006E3D4 "$TMP/useractivityd" \
  > "$OUT/disasm-SharingBTLEAdvertisementPayload.txt"
"$OBJDUMP" -d --no-show-raw-insn --start-address=0x100048CC0 --stop-address=0x100048E40 "$TMP/useractivityd" \
  > "$OUT/disasm-requestStreamEndpointData.txt"
xxd -s 0xa06ef -l 8 "$TMP/useractivityd" > "$OUT/pbpaste-command-bytes.txt"
echo "note: addresses are for useractivityd UUID $(dyld_info -arch arm64e -uuid "$UAD" | tail -1)" >> "$OUT/pbpaste-command-bytes.txt"

# Entitlements (XPC gate)
for b in "$UAD" /usr/libexec/pboard; do
  echo "=== $b"; codesign -d --entitlements - --xml "$b" 2>/dev/null | plutil -convert xml1 -o - - ; echo
done > "$OUT/entitlements.txt"

# Unified log: real Universal Clipboard sessions on this Mac (last 7 days)
/usr/bin/log show --last 7d --style compact --info \
  --predicate 'subsystem == "com.apple.useractivity" AND (category BEGINSWITH "pasteboard" OR eventMessage CONTAINS "UAPasteboardAvailable")' \
  > "$OUT/log-universal-clipboard-7d.txt"

echo "evidence written to $OUT"
