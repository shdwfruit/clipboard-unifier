# How Apple Universal Clipboard works — reverse engineered

Target: macOS 26.6.2 (25G83), arm64e. Tools: [REA](https://github.com/morluto/rea) 6.7.0
(`inspect-macho`, evidence-backed), `dyld_info` (ObjC metadata, shared-cache cstrings),
`objdump` (disassembly), `codesign` (entitlements), unified log (`log show`).
Regenerate the raw evidence with `scripts/re-collect.sh` (kept out of git: it contains
Apple binary strings and this Mac's device identifiers).

Every claim is labeled:

- **Seen**: directly observed in a binary, disassembly, entitlement, or log on this Mac.
- **Inferred**: consistent with the evidence, but not directly observed.
- **Prior work**: from published research, not re-verified here.
- **Unknown**: open question.

## The whole flow in one picture

```
 Mac (copy)                                                            iPhone (paste)
 ──────────                                                            ──────────────
 app → NSPasteboard
   └─► pboard ──XPC uasharedpasteboardmanager──► useractivityd
                                                   │ gen++ , types recorded
                                                   ▼
                                     sharingd: Handoff BLE advertisement
                                     (7-byte activity hash | opts 0x08 = pasteboard | flags)
                                     encrypted with this Mac's Handoff key ─────BLE────►  decrypts with key it
                                                                                          fetched via IDS/Rapport
                                                                                          → "remote clipboard available"
                                                                                  user pastes
                                     ◄── "Types Requested from remote" ───────────────────
                                     ◄── command "pbpaste2" (activity payload request, Rapport/IDS)
                                     ──► serviceEndpointData of SFCompanionAdvertiser
                                         ("com.apple.continuitypasteboard")
                                     ◄══ streams: Bonjour _continuity._tcp over awdl0 ═════
                                         UCStreamCoderV2: info → data / file archives → stop
 120 s after the copy: "Remove Local Types Fired" → advertisement withdrawn
```

## 1. Where it lives

| Component | Role | Evidence |
|---|---|---|
| `/usr/libexec/pboard` | Pasteboard server (CoreFoundation `CFPasteboard`). Forwards local pasteboard type changes to useractivityd and receives "remote available" updates. | **Seen**: `pboard` logs `[pasteboard-client] Sending pasteboard update to server` (108× in 7 days); entitlement `com.apple.private.coreservices.useractivityd.sharedpasteboard` |
| `UserActivity.framework/Agents/useractivityd` | Handoff daemon. Universal Clipboard is "UASharedPasteboard" inside Handoff. | **Seen**: Mach services `com.apple.coreservices.uasharedpasteboard{manager,controll,aux}.xpc` in its launchd plist; classes `UASharedPasteboardClientController`, `UCStreamCoderV2`, … |
| `/usr/libexec/sharingd` | BLE advertising/scanning, Handoff encryption keys, activity-payload requests, companion streams. | **Seen**: classes `SDActivityEncryptionManager`, `SDActivityPayloadManager`, `SDStreamHandler`; `useractivityd` may look up `com.apple.sharing.handoff.advertising` / `.scanning` |
| `UASharedPasteboardProgressUI.app` | The "Pasting from iPhone…" progress window for slow transfers. | **Seen**: launchd plist; tunables `UASharedPboardShowUIDelayInSec`, `UASharedPboardHideUIDelayInSec` |

**Seen:** the XPC path is closed to third parties. `useractivityd` gates its
shared-pasteboard XPC on the private entitlement
`com.apple.private.coreservices.useractivityd.sharedpasteboard`. Only `pboard` holds it, and
only Apple can sign `com.apple.private.*` entitlements. The one supported interface for
third-party code is the pasteboard itself (`NSPasteboard`).

## 2. Local copy → advertisement

**Seen** in the logs (one copy):

```
pboard         [pasteboard-client] [Local Pasteboard] Adding alias: public.text for type: public.utf8-plain-text
pboard         [pasteboard-client] Sending pasteboard update to server: <private>
useractivityd  [pasteboard-server] [PBOARD CONTROLLER] Got type update from client, gen: 9, <private>
useractivityd  [main] ADVERTISING:$74609a719d90c6/*46 u pb-1 2913F571-… opts={ UAPasteboardAvailable = 1; UAPasteboardVersionBit = 1; … }
```

- Each pasteboard change increments a **generation** (`currentLocalGeneration`,
  `UCRemotePasteboardGeneration`). **Seen**
- The clipboard is advertised as a pseudo-activity (`UASharedPasteboardActivityInfo` holds
  only `pasteboardTypes` and `generation`). Only type names travel up front. **Seen**
- `UAPasteboardVersionBit` **flips on each new copy** (`pb-0` → `pb-1` at `gen: 9` with the
  same activity UUID). With 1 bit to spare in the advertisement, this is how a receiver tells
  "new clipboard" apart from a re-broadcast. **Seen** (behavior), **Inferred** (purpose)

### The 9-byte advertisement payload

From disassembling `SharingBTLEAdvertisementPayload` in useractivityd (**Seen**):

```
-[… payloadBytes]          → NSData(self+0x08, 7)
-[… optionBits]            → ldrb w0, [x0, #0xf]
-[… containsPasteboard]    → ldrb w8, [x0, #0xf] ; ubfx w0, w8, #3, #1
-[… isCurrent]             → ldrb w8, [x0, #0x10] ; lsr w0, w8, #7
-[… timeOffset]            → ldrb w8, [x0, #0x10] ; and w0, w8, #0x7f
-[… advertisementPayload]  → NSData(self+0x08, 9)
```

| Byte | Meaning |
|---|---|
| 0–6 | Hashed activity type (`_LSCreateHashedBytesForAdvertisingFromString`). Logged as `$74609a719d90c6` (14 hex = 7 bytes). |
| 7 | Option bits. **Bit 3 (0x08) = clipboard has content.** The version bit is presumably another bit here (**Inferred**). |
| 8 | Bit 7 = "is current" (logged as `*`), bits 0–6 = time offset (logged as `/46`). |

The option is requested through `SFActivityAdvertiserOptionFlagCopyPasteKey` (imported from
Sharing.framework). **Seen**

**Prior work:** on air this is Apple Continuity message type `0x0C` (Handoff) in
manufacturer data `0x004C`. The layout is a clipboard status byte, a 2-byte IV/counter, a
1-byte AES-GCM auth tag, and the encrypted payload above
([Martin et al., PoPETs 2019](https://petsymposium.org/popets/2019/popets-2019-0057.php),
[furiousMAC/continuity](https://github.com/furiousMAC/continuity)). Our 0x08 option bit
matches the published "clipboard status 0x08". **Unknown:** no BLE capture was taken here
to confirm the on-air bytes for macOS 26.

## 3. Encryption keys

All **Seen** as sharingd log templates:

- Each device has its **own advertising encryption key** with a **use counter**:
  `Loaded advertising encryption key and bumped last used counter from %d by %d, then persisted key to keychain`,
  `Generating new advertising encryption key as the bumped value of the old one exceeds max value`.
- Receivers **ask the advertiser for its key** over Apple's identity services:
  `Requesting Encryption key via IDS from "%@"…`, `…via Rapport from "%@"…`,
  `Received a new Handoff encryption key from "%@" via IDS…`.
- Decryption checks an auth tag: `Unable to decrypt activity level with authTag %s identity %s…`.
- BLE pairing between the devices is "cloud pairing": `Retrying cloud pairing for devices %@`.

Consequence: a device must be an **IDS-registered (or Rapport-paired) device on the same
Apple Account** to obtain anyone's Handoff key. That is the cryptographic membership
test for Universal Clipboard.

## 4. Paste → data transfer

**Seen** (method names, strings, log templates). **Inferred** (exact ordering):

1. Paste on the receiver → `fetchRemotePasteboardTypesForProcess:` → sender logs
   `[PBOARD] Types Requested from remote <device-uuid>` (**Seen** 5× in 7 days on this Mac).
   The types request is itself an 8-byte activity-payload command, **`pbtypes!`**
   (**Seen** live: `Payload request for ad: {length = 8, bytes = 0x7062747970657321}, command: pbtypes`).
   For small clips, the data travels inline in this reply: a 13-byte text produced a
   1,787-byte reply with no follow-up request (**Inferred**, from sizes).
2. `UASharedPasteboardDataRequester requestStreamEndpointData[V2]:` sends an **8-byte
   command**, `pbpaste!` (v1) or `pbpaste2` (v2), via
   `-[SFActivityScanner activityPayloadFromDevice:forAdvertisementPayload:command:timeout:withCompletionHandler:]`.
   **Seen**: disassembly loads the 8 bytes from `0x1000a06ef` = `70 62 70 61 73 74 65 21` ("pbpaste!").
   sharingd routes the request over **Rapport** (local) or **IDS** (cloud)
   (`Requesting Handoff payload via Rapport/IDS…`).
3. The sender replies with the `serviceEndpointData` of an `SFCompanionAdvertiser` for
   service type **`com.apple.continuitypasteboard`** with `setSupportsStreams:YES`.
4. The receiver calls `getContinuationStreamsWithEndpointData:completionHandler:` and gets an
   `NSInputStream`/`NSOutputStream` pair. sharingd resolves it via Bonjour **`_continuity._tcp.`**
   (`Bonjour browser failed to find awdl0 interface`, `CompanionStreamConnectResponse`), so
   it runs over **AWDL**, peer-to-peer Wi-Fi.
   **Inferred:** sharingd authenticates the stream with certificate identities
   (`SecTrust`, `### No identity for %@`). Apple's Platform Security guide describes Handoff
   transfers as encrypted peer-to-peer Wi-Fi. AWDL itself is analyzed in
   [Stute et al., USENIX Security 2019](https://www.usenix.org/conference/usenixsecurity19/presentation/stute).
5. Coder negotiation: `UAPBStreamCoderV1` (raw chunked) or `UCStreamCoderV2` (tagged
   messages: `PasteboardInfoRequest/Response`, `PasteboardDataRequest/Response`,
   `PasteboardFileRequest/Response`, `AuxInfo`, `TotalSendSize`, `Heartbeat`, `Stop`).
   The sender's side is `advertiser:didReceiveInputStream:outputStream:`.
6. Payload model (`UASharedPasteboardInfo` keys in UserActivity.framework):
   `Items[] → Types[] → {Type, UUID, Offset, Size, DataFile, PrefRep, TypeHint}`, plus
   `DataSize` and `DataFile`. All type representations travel as one blob with per-type
   offsets. Files are sent as archives (`fileURLForArchiveWithUUID:`; useractivityd links
   `libParallelCompression` and `libbz2`). Small payloads may skip streams
   (`UASharedPboardEmbeddedPayloadSizeLimitInBytes`) (**Inferred** from the key name).
7. The receiver writes the content to its pasteboard, marked with type
   **`com.apple.is-remote-clipboard`**. **Seen**: this string is in CoreFoundation,
   UserActivity.framework, and SpotlightUIShared (macOS 26 Spotlight clipboard history).

## 5. Lifetime and policy

- **Expiry is 120 s.** `Got type update` 18:19:09.08 → `Remove Local Types Fired` 18:21:09.08,
  18:23:13.88 → 18:25:13.88, 12:46:07.47 → 12:48:07.78. **Seen**
- **Low Power Mode stops all advertising, clipboard included, but not receiving.** **Seen**:
  useractivityd observes `com.apple.system.lowpowermode` and logs
  ` ** LowPowerMode is enabled so no activity will be advertised. ** `, with override
  key `EnableHandoffInPowerSaverMode`. A device in Low Power Mode can still paste from
  others, but its own copies never reach them. **Inferred**: iOS shares this code path.
- Universal Clipboard is **off during Screen Sharing sessions**:
  `##### Screen Sharing session in progress, Universal Clipboard is disabled`. **Seen**
- Advertising is delayed on screen saver, screen dim, and device lock
  (`ScreenSaverPasteboardAdvertisementDelay`, `ScreenDimPasteboardAdvertisementDelay`,
  `DeviceLockPasteboardAdvertisementDelay`). **Seen** (names)
- Other tunables (names **Seen**; domains and defaults **Unknown**): `ClipboardSharingEnabled`,
  `UASharedPboardLocalAdvertisementTime`, `UASharedPboardRemoteAvailableTimeout`,
  `PasteRequestTimeout`, `PasteboardStreamWatchdogTimeout`, `DEBUGForceNewPayloadEncoder`.
- Opt-out marker: CoreFoundation carries `com.apple.pboard.make-generation-local`.
  **Inferred**: this is what `NSPasteboardContentsCurrentHostOnly` sets, so the copy never
  advertises.

## 6. Why Windows/Linux can't simply join

| Wall | Detail | Status |
|---|---|---|
| **Identity** | Getting Handoff keys requires being an IDS-registered device on the Apple Account (§3). IDS registration needs Apple-hardware validation data. Beeper Mini / [pypush](https://github.com/JJTech0130/pypush) hit this wall, and Apple shut it down in Dec 2023. | Seen + Prior work |
| **Entitlements** | Even on a Mac, third-party code can't talk to useractivityd's pasteboard XPC (§1). | Seen |
| **Radio** | Bulk data goes over AWDL. Linux has [OWL](https://github.com/seemoo-lab/owl), but only on Wi-Fi chips with monitor mode and injection. Windows has no equivalent. | Seen + Prior work |
| **Fragility** | Keys rotate (counter max), and Apple can revoke registrations server-side. | Seen |

Research directions, not built:

- **Observe-only on Linux:** a BLE sniffer could detect "an iPhone has a new clipboard" from
  the Continuity 0x0C clipboard byte (plaintext per prior work), but can't fetch the data.
- **Full peer:** pypush-style IDS registration + OWL + the TLS stream protocol. This is high
  effort, likely against Apple's terms, and likely to break.

## 7. What clipbridge does with this

The Mac is already a legitimate Universal Clipboard peer, so clipbridge uses the one
interface Apple leaves open, `NSPasteboard`:

- **iPhone → PC:** an iPhone copy lands on the Mac pasteboard flagged
  `com.apple.is-remote-clipboard`. clipbridge sees the `changeCount` bump, reads it (which
  triggers the AWDL fetch), and sends it to peers.
- **PC → iPhone:** clipbridge writes received content as an ordinary local copy. pboard →
  useractivityd → BLE advertisement, so the iPhone can paste it within 120 s.
  **Seen** in the Mac ↔ Linux e2e run:
  - clipbridge wrote the Linux text at generation 51 (19:03:09.381), and
    `ADVERTISING … UAPasteboardAvailable = 1 … pb-1` followed at 19:03:10.061.
  - The next write (an image, gen 52) flipped the bit to `pb-0` within 3 ms.
  - Advertisement updates appear coalesced to at most one per second.
- `--local-only` writes with `NSPasteboardContentsCurrentHostOnly`, keeping bridged
  content off the iPhone.
- Respects `org.nspasteboard.ConcealedType` and `TransientType`, which are also present in
  the system's shared cache.

## 8. Receive side, observed (iPhone → Mac)

**Seen** on 2026-10-18. An iPhone copy appears on the Mac pasteboard *immediately*, with
`com.apple.is-remote-clipboard` (`scripts/pbwatch.js` printed `remote=true`), before
anything pastes. Each iPhone copy bumps `changeCount` separately. Three copies 5–7 s apart
gave 64, 65, and 66, each with these types:

```
[public.utf8-plain-text, NSStringPboardType, public.text, com.apple.is-remote-clipboard]
```

The marker is an ordinary pasteboard type on the item. `public.text` is the alias the
sender adds (`Adding alias: public.text for type: public.utf8-plain-text`).

The types are fetched eagerly; the data is fetched when something reads it:

```
19:25:50.787 [PBOARD] Notify client of new remote pboard and reset timmer
19:25:50.789 [pasteboard-fetch] Created remote generation 11D686AC-…
19:25:50.789 [PBOARD CONTROLLER] Sending client xpc remote available update: YES
19:25:50.789 [REMOTE GEN] Fetching remote type info
19:25:50.821 [REMOTE GEN] Received type info                       ← 32 ms over Rapport
19:25:50.823 [REMOTE GEN] Setting protocol version: 2
19:25:52.134 [PBOARD CONTROLLER] Requesting data from remote device
19:25:52.161 [FILE MANAGER] Creating file for remote pasteboard blob at path: <private>
19:26:18.498 [PBOARD CONTROLLER] Sending client xpc remote available update: NO
```

## 9. Troubleshooting: one direction dead

**Seen**, same day. Mac → iPhone worked, but iPhone → Mac didn't. The Mac's logs showed
iPhone Handoff advertisements arriving and decrypting (`Successfully decrypted advertisement`),
but every one had `SFActivityAdvertiserOptionFlagCopyPasteKey = 0`. A live capture during an
iPhone copy showed *no* new advertisement at all, so the iPhone wasn't announcing copies.

- **Not the cause:** Low Power Mode was off, and turning Handoff off and on didn't help.
- **Fix:** restarting the iPhone.

To check which side is at fault, run:

```sh
log show --last 1h --style compact --predicate \
  'process == "useractivityd" AND eventMessage CONTAINS "RECEIVED advertisement"'
```

- **No `CopyPasteKey = 1` after an iPhone copy:** the iPhone isn't advertising. Restart it.
- **`CopyPasteKey = 1` but no `Created remote generation`:** the Mac is dropping it.

## 10. Open questions

- On-air BLE bytes and the `UCStreamCoderV2` framing were not captured or decoded.
- Whether infrastructure Wi-Fi can replace AWDL for the stream.
- macOS 26 pasteboard privacy: `NSPasteboard.accessBehavior` was `2` (always allow) here.
  A background agent may get prompted once (Privacy & Security → Paste from Other Apps).
