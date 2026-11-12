//! Takes the window in front of every other app's with ScreenCaptureKit and reads its text through
//! the accessibility API. ScreenCaptureKit is loaded when first used, since the app also starts on
//! macOS versions that do not have it.

use std::ffi::c_void;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use block2::RcBlock;
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use objc2::rc::{autoreleasepool, Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, AnyThread};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImageCompressionFactor};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSDictionary, NSNumber, NSRect, NSString};

use super::{Element, Missed, Shot, Text};

/// The longest side of a picture, in pixels. Agents shrink larger ones anyway.
const LONGEST: f64 = 2560.;
const JPEG_QUALITY: f64 = 0.85;
/// How long ScreenCaptureKit gets to answer.
const CAPTURE_WAIT: Duration = Duration::from_secs(10);

/// How much of a window's text is read: the time, the elements looked at, those listed, their
/// characters in all and in one, and how deep.
const TEXT_TIME: Duration = Duration::from_millis(600);
const LOOKED_AT: usize = 4000;
const LISTED: usize = 400;
const CHARACTERS: usize = 24_000;
const ONE_ELEMENT: usize = 2000;
const DEPTH: usize = 60;
/// How long one app is waited on for one accessibility answer, in seconds.
const ANSWER_WAIT: f32 = 0.2;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

const ON_SCREEN_ONLY: u32 = 1 << 0;
const EXCLUDE_DESKTOP: u32 = 1 << 4;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> i32;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> bool;
}

const AX_POINT: u32 = 1;
const AX_SIZE: u32 = 2;

/// A window another app has on screen.
struct Found {
    id: u32,
    pid: i32,
    app: String,
    title: String,
    /// x, y, width, height in points, from the top left of the main screen.
    bounds: [f64; 4],
}

pub fn capture() -> Result<Shot, Missed> {
    if !kit_loaded() {
        return Err(Missed::Unsupported);
    }
    // SAFETY: both take nothing; the request only shows macOS's prompt the first time.
    if !unsafe { CGPreflightScreenCaptureAccess() } {
        unsafe { CGRequestScreenCaptureAccess() };
        return Err(Missed::NotAllowed);
    }
    let found = front_window().ok_or(Missed::NoWindow)?;
    let (jpeg, width, height, scale) = picture(&found).map_err(Missed::Failed)?;
    let text = read_text(&found);
    Ok(Shot {
        jpeg,
        app: found.app,
        title: found.title,
        width,
        height,
        scale,
        text,
    })
}

fn kit_loaded() -> bool {
    static LOADED: OnceLock<bool> = OnceLock::new();
    *LOADED.get_or_init(|| {
        let path = c"/System/Library/Frameworks/ScreenCaptureKit.framework/ScreenCaptureKit";
        // SAFETY: loading a system framework by its full path runs only Apple's own code.
        let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY) };
        !handle.is_null() && AnyClass::get(c"SCScreenshotManager").is_some()
    })
}

/// The window in front of every other app's, which is the one the person was in when they came
/// to Sikemux, or is in now when they pressed the shortcut in another app.
fn front_window() -> Option<Found> {
    // SAFETY: the options are valid, and the array returned is ours to release.
    let list = unsafe { CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0) };
    if list.is_null() {
        return None;
    }
    // SAFETY: a CFArray of CFDictionary, returned retained.
    let list: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(list) };
    let ours = std::process::id() as i64;
    list.iter().find_map(|info| {
        // SAFETY: each item of the window list is a dictionary.
        let info: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(info.as_CFTypeRef() as CFDictionaryRef) };
        let number = |key: &'static str| {
            info.find(CFString::from_static_string(key))
                .and_then(|value| value.downcast::<CFNumber>())
                .and_then(|number| number.to_f64())
        };
        let text = |key: &'static str| {
            info.find(CFString::from_static_string(key))
                .and_then(|value| value.downcast::<CFString>())
                .map(|text| plain(&text))
                .unwrap_or_default()
        };
        let pid = number("kCGWindowOwnerPID")? as i64;
        if number("kCGWindowLayer")? != 0. || pid == ours || number("kCGWindowAlpha")? <= 0. {
            return None;
        }
        let bounds = info
            .find(CFString::from_static_string("kCGWindowBounds"))
            .and_then(|value| value.downcast::<CFDictionary>())?;
        // SAFETY: the bounds are a dictionary of numbers keyed by strings.
        let bounds: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bounds.as_concrete_TypeRef()) };
        let side = |key: &'static str| {
            bounds
                .find(CFString::from_static_string(key))
                .and_then(|value| value.downcast::<CFNumber>())
                .and_then(|number| number.to_f64())
        };
        let bounds = [side("X")?, side("Y")?, side("Width")?, side("Height")?];
        if bounds[2] < 40. || bounds[3] < 40. {
            return None;
        }
        Some(Found {
            id: number("kCGWindowNumber")? as u32,
            pid: pid as i32,
            app: text("kCGWindowOwnerName"),
            title: text("kCGWindowName"),
            bounds,
        })
    })
}

/// The window's picture as a JPEG, with its size in pixels and pixels to a point.
fn picture(found: &Found) -> Result<(Vec<u8>, u32, u32, f64), String> {
    let (sent, got) = mpsc::channel();
    let id = found.id;
    let longest = found.bounds[2].max(found.bounds[3]);
    let handler = RcBlock::new(move |content: *mut AnyObject, error: *mut AnyObject| {
        let sent = sent.clone();
        autoreleasepool(|_| {
            // SAFETY: ScreenCaptureKit hands either the shareable content or an error.
            if let Err(error) = unsafe { shoot(content, error, id, longest, sent.clone()) } {
                let _ = sent.send(Err(error));
            }
        });
    });
    let Some(content) = AnyClass::get(c"SCShareableContent") else {
        return Err("ScreenCaptureKit is missing".to_owned());
    };
    // SAFETY: the class method takes two flags and a block it calls once.
    let _: () = unsafe {
        msg_send![content, getShareableContentExcludingDesktopWindows: true, onScreenWindowsOnly: true, completionHandler: &*handler]
    };
    got.recv_timeout(CAPTURE_WAIT)
        .map_err(|_| "the window was not taken in time".to_owned())?
}

type Taken = Result<(Vec<u8>, u32, u32, f64), String>;

/// Takes window `id` out of the shareable `content`, at no more than [`LONGEST`] pixels a side.
///
/// # Safety
/// `content` is an `SCShareableContent` or null, and `error` an `NSError` or null.
unsafe fn shoot(
    content: *mut AnyObject,
    error: *mut AnyObject,
    id: u32,
    longest: f64,
    sent: mpsc::Sender<Taken>,
) -> Result<(), String> {
    let Some(content) = (unsafe { content.as_ref() }) else {
        return Err(unsafe { described(error) });
    };
    let windows: Retained<AnyObject> = unsafe { msg_send![content, windows] };
    let count: usize = unsafe { msg_send![&*windows, count] };
    let window = (0..count)
        .map(|at| -> Retained<AnyObject> { unsafe { msg_send![&*windows, objectAtIndex: at] } })
        .find(|window| {
            let its: u32 = unsafe { msg_send![&**window, windowID] };
            its == id
        })
        .ok_or("the window closed")?;
    let (Some(filter), Some(config), Some(manager)) = (
        AnyClass::get(c"SCContentFilter"),
        AnyClass::get(c"SCStreamConfiguration"),
        AnyClass::get(c"SCScreenshotManager"),
    ) else {
        return Err("ScreenCaptureKit is missing".to_owned());
    };
    let filter: Allocated<AnyObject> = unsafe { msg_send![filter, alloc] };
    let filter: Retained<AnyObject> =
        unsafe { msg_send![filter, initWithDesktopIndependentWindow: &*window] };
    let frame: NSRect = unsafe { msg_send![&*window, frame] };
    let native: f32 = unsafe { msg_send![&*filter, pointPixelScale] };
    let scale = f64::from(native).min(LONGEST / longest.max(1.));
    let width = (frame.size.width * scale).round().max(1.) as usize;
    let height = (frame.size.height * scale).round().max(1.) as usize;
    let config: Retained<AnyObject> = unsafe { msg_send![config, new] };
    let _: () = unsafe { msg_send![&*config, setWidth: width] };
    let _: () = unsafe { msg_send![&*config, setHeight: height] };
    let _: () = unsafe { msg_send![&*config, setShowsCursor: false] };
    let taken = RcBlock::new(move |image: *mut CGImage, error: *mut AnyObject| {
        autoreleasepool(|_| {
            // SAFETY: ScreenCaptureKit hands either the picture or an error.
            let jpeg = match unsafe { image.as_ref() } {
                Some(image) => jpeg(image),
                None => Err(unsafe { described(error) }),
            };
            let _ = sent.send(jpeg.map(|jpeg| (jpeg, width as u32, height as u32, scale)));
        });
    });
    let _: () = unsafe {
        msg_send![manager, captureImageWithFilter: &*filter, configuration: &*config, completionHandler: &*taken]
    };
    Ok(())
}

/// # Safety
/// `error` is an `NSError` or null.
unsafe fn described(error: *mut AnyObject) -> String {
    match unsafe { error.as_ref() } {
        Some(error) => {
            let said: Retained<NSString> = unsafe { msg_send![error, localizedDescription] };
            said.to_string()
        }
        None => "macOS gave no reason".to_owned(),
    }
}

fn jpeg(image: &CGImage) -> Result<Vec<u8>, String> {
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
    let quality = NSNumber::new_f64(JPEG_QUALITY);
    // SAFETY: the key is AppKit's own, and its value a number as it expects.
    let properties = NSDictionary::<NSString, AnyObject>::from_slices(
        &[unsafe { NSImageCompressionFactor }],
        &[&*quality],
    );
    // SAFETY: the properties hold only the compression factor, a number.
    unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::JPEG, &properties) }
        .map(|data| data.to_vec())
        .ok_or_else(|| "the picture would not encode".to_owned())
}

/// What the window shows as text, when macOS lets Sikemux read it.
fn read_text(found: &Found) -> Text {
    // SAFETY: takes nothing.
    if !unsafe { AXIsProcessTrusted() } {
        return Text::NotAllowed;
    }
    // SAFETY: returns a new element for the process, ours to release.
    let app = unsafe { CFType::wrap_under_create_rule(AXUIElementCreateApplication(found.pid)) };
    // SAFETY: `app` is an application element.
    unsafe { AXUIElementSetMessagingTimeout(app.as_CFTypeRef(), ANSWER_WAIT) };
    let Some(window) = matching_window(&app, found) else {
        return Text::Unread;
    };
    let origin =
        point(&window, "AXPosition", AX_POINT).unwrap_or([found.bounds[0], found.bounds[1]]);
    let deadline = Instant::now() + TEXT_TIME;
    let (mut elements, mut more, mut characters, mut looked) = (Vec::new(), 0, 0, 0);
    let mut waiting: Vec<(CFType, usize)> = children(&window)
        .into_iter()
        .rev()
        .map(|child| (child, 1))
        .collect();
    while let Some((element, depth)) = waiting.pop() {
        if looked == LOOKED_AT || Instant::now() > deadline {
            break;
        }
        looked += 1;
        if let Some(text) = said(&element) {
            if elements.len() < LISTED && characters + text.len() <= CHARACTERS {
                characters += text.len();
                let role = string(&element, "AXRole").unwrap_or_default();
                let at = point(&element, "AXPosition", AX_POINT).unwrap_or(origin);
                let size = point(&element, "AXSize", AX_SIZE).unwrap_or_default();
                elements.push(Element {
                    role: role.strip_prefix("AX").unwrap_or(&role).to_owned(),
                    text,
                    frame: [at[0] - origin[0], at[1] - origin[1], size[0], size[1]],
                });
            } else {
                more += 1;
            }
        }
        if depth < DEPTH {
            waiting.extend(
                children(&element)
                    .into_iter()
                    .rev()
                    .map(|child| (child, depth + 1)),
            );
        }
    }
    Text::Read { elements, more }
}

/// The app's window that sits where the picture's did, else the one with its title, else its
/// focused one.
fn matching_window(app: &CFType, found: &Found) -> Option<CFType> {
    let windows = attribute(app, "AXWindows")
        .and_then(|windows| windows.downcast::<CFArray>())
        .map(|windows| {
            // SAFETY: the windows attribute is an array of elements.
            let windows: CFArray<CFType> =
                unsafe { CFArray::wrap_under_get_rule(windows.as_concrete_TypeRef()) };
            windows
                .iter()
                .map(|window| window.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let placed = |window: &&CFType| {
        let at = point(window, "AXPosition", AX_POINT);
        let size = point(window, "AXSize", AX_SIZE);
        let near = |a: f64, b: f64| (a - b).abs() < 2.;
        matches!((at, size), (Some(at), Some(size))
            if near(at[0], found.bounds[0]) && near(at[1], found.bounds[1])
                && near(size[0], found.bounds[2]) && near(size[1], found.bounds[3]))
    };
    let titled = |window: &&CFType| string(window, "AXTitle").as_deref() == Some(&found.title);
    windows
        .iter()
        .find(placed)
        .or_else(|| windows.iter().find(titled))
        .cloned()
        .or_else(|| attribute(app, "AXFocusedWindow"))
}

fn attribute(element: &CFType, name: &'static str) -> Option<CFType> {
    let name = CFString::from_static_string(name);
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: `element` is an accessibility element, and a value it returns is ours to release.
    let error = unsafe {
        AXUIElementCopyAttributeValue(
            element.as_CFTypeRef(),
            name.as_concrete_TypeRef(),
            &mut value,
        )
    };
    (error == 0 && !value.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(value) })
}

fn string(element: &CFType, name: &'static str) -> Option<String> {
    attribute(element, name)?
        .downcast::<CFString>()
        .map(|text| plain(&text))
}

fn children(element: &CFType) -> Vec<CFType> {
    let Some(children) =
        attribute(element, "AXChildren").and_then(|value| value.downcast::<CFArray>())
    else {
        return Vec::new();
    };
    // SAFETY: the children attribute is an array of elements.
    let children: CFArray<CFType> =
        unsafe { CFArray::wrap_under_get_rule(children.as_concrete_TypeRef()) };
    children.iter().map(|child| child.clone()).collect()
}

/// A point or size attribute, as two numbers.
fn point(element: &CFType, name: &'static str, kind: u32) -> Option<[f64; 2]> {
    let value = attribute(element, name)?;
    let mut pair = [0f64; 2];
    // SAFETY: a CGPoint and a CGSize are each two doubles, which `pair` holds.
    let read = unsafe { AXValueGetValue(value.as_CFTypeRef(), kind, pair.as_mut_ptr().cast()) };
    read.then_some(pair)
}

/// The words an element shows: its value when that is text, else its title or description.
fn said(element: &CFType) -> Option<String> {
    ["AXValue", "AXTitle", "AXDescription"]
        .into_iter()
        .filter_map(|name| string(element, name))
        .map(|text| text.trim().to_owned())
        .find(|text| !text.is_empty())
        .map(|text| match text.char_indices().nth(ONE_ELEMENT) {
            Some((cut, _)) => format!("{}…", &text[..cut]),
            None => text,
        })
}

/// Text without the invisible marks some apps set their words' direction with.
fn plain(text: &CFString) -> String {
    text.to_string().replace(['\u{200e}', '\u{200f}'], "")
}
