//! macOS backend: talks to `NSPasteboard` directly so we can see pasteboard *types*, which
//! is how Universal Clipboard content and password-manager content are recognised.

use super::{ClipboardBackend, WriteOptions};
use crate::engine::Observed;
use crate::proto::Content;
use anyhow::{Result, bail};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::NSObjectProtocol;
use objc2::sel;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardAccessBehavior,
    NSPasteboardContentsOptions,
};
use objc2_foundation::{NSData, NSDictionary, NSString};

/// Pasteboard types that mark content which arrived from another device through
/// Universal Clipboard (see docs/universal-clipboard-re.md).
pub const REMOTE_MARKER_TYPES: &[&str] = &["com.apple.is-remote-clipboard"];

/// nspasteboard.org conventions used by password managers.
const CONCEALED_TYPES: &[&str] = &[
    "org.nspasteboard.ConcealedType",
    "org.nspasteboard.TransientType",
];

/// Added to everything we write, so our own writes are recognisable.
const APPLIED_MARKER: &str = "dev.clipbridge.applied";

const TEXT: &str = "public.utf8-plain-text";
const PNG: &str = "public.png";
const TIFF: &str = "public.tiff";
/// Image types we read, in order of preference. Non-PNG types are converted to PNG.
const IMAGE_TYPES: &[&str] = &[PNG, TIFF, "public.jpeg", "public.heic"];

pub struct MacClipboard {
    opts: WriteOptions,
    last_count: isize,
    /// changeCount right after our last write.
    our_count: Option<isize>,
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

impl MacClipboard {
    pub fn new(opts: WriteOptions) -> Self {
        let last_count = NSPasteboard::generalPasteboard().changeCount();
        Self {
            opts,
            last_count,
            our_count: None,
        }
    }
}

fn types(pb: &NSPasteboard) -> Vec<String> {
    pb.types()
        .map(|arr| arr.iter().map(|t| t.to_string()).collect())
        .unwrap_or_default()
}

/// macOS 26 pasteboard privacy setting for this process (`None` on older macOS).
fn access_behavior(pb: &NSPasteboard) -> Option<NSPasteboardAccessBehavior> {
    pb.respondsToSelector(sel!(accessBehavior))
        .then(|| pb.accessBehavior())
}

fn describe_access(b: NSPasteboardAccessBehavior) -> &'static str {
    match b {
        NSPasteboardAccessBehavior::Default => "default",
        NSPasteboardAccessBehavior::Ask => "ask",
        NSPasteboardAccessBehavior::AlwaysAllow => "always allow",
        NSPasteboardAccessBehavior::AlwaysDeny => "always deny",
        _ => "unknown",
    }
}

fn to_png(data: &NSData) -> Option<Vec<u8>> {
    let rep = NSBitmapImageRep::imageRepWithData(data)?;
    let props = NSDictionary::new();
    // SAFETY: an empty properties dictionary is valid for any storage type.
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) };
    png.map(|d| d.to_vec())
}

fn read_content(pb: &NSPasteboard, types: &[String]) -> Option<Content> {
    let has = |t: &str| types.iter().any(|x| x == t);
    if has(TEXT) {
        return pb
            .stringForType(&ns(TEXT))
            .map(|s| Content::Text(s.to_string()));
    }
    for &ty in IMAGE_TYPES.iter().filter(|t| has(t)) {
        let Some(data) = pb.dataForType(&ns(ty)) else {
            continue;
        };
        let png = if ty == PNG {
            Some(data.to_vec())
        } else {
            to_png(&data)
        };
        if let Some(png) = png {
            return Some(Content::Png(png));
        }
    }
    None
}

impl ClipboardBackend for MacClipboard {
    fn name(&self) -> &'static str {
        "macOS NSPasteboard"
    }

    fn poll(&mut self) -> Result<Option<Observed>> {
        autoreleasepool(|_| {
            let pb = NSPasteboard::generalPasteboard();
            let count = pb.changeCount();
            if count == self.last_count {
                return Ok(None);
            }
            self.last_count = count;
            let types = types(&pb);
            let has = |t: &str| types.iter().any(|x| x == t);
            let mut obs = Observed {
                ours: self.our_count == Some(count) || has(APPLIED_MARKER),
                concealed: CONCEALED_TYPES.iter().any(|t| has(t)),
                via_universal_clipboard: REMOTE_MARKER_TYPES.iter().any(|t| has(t)),
                content: None,
            };
            if !obs.ours && !obs.concealed {
                // For Universal Clipboard content this read is what pulls the data from the
                // other device, so it can take a moment.
                obs.content = read_content(&pb, &types);
            }
            Ok(Some(obs))
        })
    }

    fn write(&mut self, content: &Content) -> Result<()> {
        autoreleasepool(|_| {
            let pb = NSPasteboard::generalPasteboard();
            if self.opts.local_only {
                pb.prepareForNewContentsWithOptions(NSPasteboardContentsOptions::CurrentHostOnly);
            } else {
                pb.clearContents();
            }
            let ok = match content {
                Content::Text(s) => pb.setString_forType(&ns(s), &ns(TEXT)),
                Content::Png(bytes) => {
                    let data = NSData::with_bytes(bytes);
                    let ok = pb.setData_forType(Some(&data), &ns(PNG));
                    // Older apps only read TIFF.
                    if let Some(tiff) =
                        NSBitmapImageRep::imageRepWithData(&data).and_then(|r| r.TIFFRepresentation())
                    {
                        pb.setData_forType(Some(&tiff), &ns(TIFF));
                    }
                    ok
                }
            };
            pb.setData_forType(Some(&NSData::new()), &ns(APPLIED_MARKER));
            let count = pb.changeCount();
            self.our_count = Some(count);
            self.last_count = count;
            if !ok {
                bail!("NSPasteboard rejected the {}", content.kind());
            }
            Ok(())
        })
    }

    fn diagnostics(&mut self) -> Vec<String> {
        autoreleasepool(|_| {
            let pb = NSPasteboard::generalPasteboard();
            let access = access_behavior(&pb).map_or("n/a (before macOS 26)".to_string(), |b| {
                format!("{} ({})", describe_access(b), b.0)
            });
            vec![
                format!("pasteboard access behavior: {access}"),
                format!("pasteboard changeCount: {}", pb.changeCount()),
                format!("current pasteboard types: {:?}", types(&pb)),
            ]
        })
    }

    fn startup_warnings(&mut self) -> Vec<String> {
        autoreleasepool(|_| {
            let pb = NSPasteboard::generalPasteboard();
            match access_behavior(&pb) {
                Some(b @ (NSPasteboardAccessBehavior::Ask | NSPasteboardAccessBehavior::AlwaysDeny)) => {
                    vec![format!(
                        "pasteboard access is \"{}\": macOS may block clipboard reads — allow \
                         clipbridge in System Settings → Privacy & Security → Paste from Other Apps",
                        describe_access(b)
                    )]
                }
                _ => Vec::new(),
            }
        })
    }
}
