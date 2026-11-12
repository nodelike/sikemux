//! The Mac apps a host's processes belong to: each process counts as the outermost `.app` its
//! program is in, so an app's helpers count as the app, which is named and drawn as the Finder
//! shows it.

use crate::server::parts::Parts;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sikemux_process::procs::Usage;
use sikemux_process::sync::Locked;

use super::super::{Core, CoreResult};

pub(super) struct App {
    /// What the app is told apart by: its bundle id, else where it is.
    pub key: String,
    pub name: String,
    bundle: PathBuf,
}

/// Which app each process is in, looked up once per process, and each app once per bundle.
#[derive(Default)]
pub(super) struct Apps {
    of_process: HashMap<(u32, u64), Seen>,
    #[cfg(target_os = "macos")]
    of_bundle: HashMap<PathBuf, Option<Arc<App>>>,
}

/// A process as last seen: its name, which running another program changes, and its app.
struct Seen {
    name: String,
    app: Option<Arc<App>>,
}

impl Apps {
    /// The app of each of `usages`, in order, with the apps met for the first time.
    pub fn of(&mut self, usages: &[Usage]) -> (Vec<Option<Arc<App>>>, Vec<Arc<App>>) {
        let mut met = Vec::new();
        if !cfg!(target_os = "macos") {
            return (vec![None; usages.len()], met);
        }
        let mut kept = HashMap::with_capacity(usages.len());
        let apps = (usages.iter())
            .map(|usage| {
                let process = (usage.pid, usage.started_ms);
                let seen = match self.of_process.remove(&process) {
                    Some(seen) if seen.name == usage.name => seen,
                    _ => Seen {
                        name: usage.name.clone(),
                        app: self.look_up(usage.pid, &mut met),
                    },
                };
                let app = seen.app.clone();
                kept.insert(process, seen);
                app
            })
            .collect();
        self.of_process = kept;
        (apps, met)
    }

    #[cfg(target_os = "macos")]
    fn look_up(&mut self, pid: u32, met: &mut Vec<Arc<App>>) -> Option<Arc<App>> {
        let program = sikemux_process::procs::program(pid)?;
        let bundle = (program.ancestors())
            .filter(|folder| {
                folder
                    .extension()
                    .is_some_and(|extension| extension == "app")
            })
            .last()?;
        if let Some(app) = self.of_bundle.get(bundle) {
            return app.clone();
        }
        let app = mac::named(bundle).map(|(key, name)| {
            Arc::new(App {
                key,
                name,
                bundle: bundle.to_owned(),
            })
        });
        met.extend(app.clone());
        self.of_bundle.insert(bundle.to_owned(), app.clone());
        app
    }

    #[cfg(not(target_os = "macos"))]
    fn look_up(&mut self, _pid: u32, _met: &mut Vec<Arc<App>>) -> Option<Arc<App>> {
        None
    }
}

/// The apps met on this host by key, each with its icon once a client has asked for it.
#[derive(Default)]
pub(crate) struct Icons(Mutex<HashMap<String, Icon>>);

struct Icon {
    bundle: PathBuf,
    /// A PNG in base64, or none where the app has no icon to draw.
    drawn: Option<Option<Arc<str>>>,
}

impl Icons {
    pub(super) fn meet(&self, apps: Vec<Arc<App>>) {
        let mut icons = self.0.locked();
        for app in apps {
            icons.entry(app.key.clone()).or_insert_with(|| Icon {
                bundle: app.bundle.clone(),
                drawn: None,
            });
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppIconParams {
    pub app: String,
}

/// The icon of the app `app` names, as a PNG in base64, drawn the first time it is asked for.
pub(crate) async fn icon(core: &Arc<Core>, params: AppIconParams) -> CoreResult<Option<String>> {
    let bundle = {
        let icons = core.host_watch().icons.0.locked();
        let Some(icon) = icons.get(&params.app) else {
            return Ok(None);
        };
        if let Some(drawn) = &icon.drawn {
            return Ok(drawn.as_deref().map(str::to_owned));
        }
        icon.bundle.clone()
    };
    let drawn = super::super::connection::blocking(move || Ok(png(&bundle))).await?;
    let drawn: Option<Arc<str>> = drawn.map(|png| {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(png).into()
    });
    if let Some(icon) = core.host_watch().icons.0.locked().get_mut(&params.app) {
        icon.drawn = Some(drawn.clone());
    }
    Ok(drawn.as_deref().map(str::to_owned))
}

#[cfg(target_os = "macos")]
use mac::icon as png;

#[cfg(not(target_os = "macos"))]
fn png(_bundle: &std::path::Path) -> Option<Vec<u8>> {
    None
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::Path;
    use std::sync::OnceLock;

    use objc2::msg_send;
    use objc2::rc::{autoreleasepool, Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::{
        ns_string, NSBundle, NSData, NSDictionary, NSPoint, NSRect, NSSize, NSString,
    };

    /// The app's key and the name the Finder shows it by.
    pub(super) fn named(bundle: &Path) -> Option<(String, String)> {
        autoreleasepool(|_| {
            let path = NSString::from_str(bundle.to_str()?);
            let info = NSBundle::bundleWithPath(&path)?;
            let named = |key: &NSString| {
                info.objectForInfoDictionaryKey(key)
                    .and_then(|value| value.downcast::<NSString>().ok())
                    .map(|value| value.to_string())
                    .filter(|value| !value.trim().is_empty())
            };
            let name = named(ns_string!("CFBundleDisplayName"))
                .or_else(|| named(ns_string!("CFBundleName")))
                .or_else(|| Some(bundle.file_stem()?.to_string_lossy().into_owned()))?;
            let key = (info.bundleIdentifier())
                .map(|id| id.to_string())
                .unwrap_or_else(|| bundle.to_string_lossy().into_owned());
            Some((key, name))
        })
    }

    /// AppKit, which the core loads only once an app's icon is first wanted.
    fn app_kit() -> bool {
        static LOADED: OnceLock<bool> = OnceLock::new();
        *LOADED.get_or_init(|| {
            // SAFETY: the path is a C string, and AppKit stays loaded for the process's life.
            let handle = unsafe {
                libc::dlopen(
                    c"/System/Library/Frameworks/AppKit.framework/AppKit".as_ptr(),
                    libc::RTLD_LAZY,
                )
            };
            !handle.is_null()
        })
    }

    /// How many pixels wide an app's icon is drawn for clients.
    const SIDE: f64 = 64.;

    /// The bundle's icon as a PNG.
    pub(super) fn icon(bundle: &Path) -> Option<Vec<u8>> {
        if !app_kit() {
            return None;
        }
        let workspace = AnyClass::get(c"NSWorkspace")?;
        let bitmap = AnyClass::get(c"NSBitmapImageRep")?;
        let context = AnyClass::get(c"NSGraphicsContext")?;
        autoreleasepool(|_| {
            let path = NSString::from_str(bundle.to_str()?);
            let pixels = SIDE as isize;
            let square = NSRect::new(NSPoint::new(0., 0.), NSSize::new(SIDE, SIDE));
            // SAFETY: each message is one AppKit documents for its receiver, with the argument
            // and answer types it declares; the bitmap is drawn into only while its context is
            // this thread's current one.
            unsafe {
                let shared: Retained<AnyObject> = msg_send![workspace, sharedWorkspace];
                let image: Option<Retained<AnyObject>> = msg_send![&shared, iconForFile: &*path];
                let image = image?;
                let blank: Allocated<AnyObject> = msg_send![bitmap, alloc];
                let rep: Option<Retained<AnyObject>> = msg_send![
                    blank,
                    initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>(),
                    pixelsWide: pixels,
                    pixelsHigh: pixels,
                    bitsPerSample: 8isize,
                    samplesPerPixel: 4isize,
                    hasAlpha: Bool::YES,
                    isPlanar: Bool::NO,
                    colorSpaceName: ns_string!("NSDeviceRGBColorSpace"),
                    bytesPerRow: 0isize,
                    bitsPerPixel: 0isize
                ];
                let rep = rep?;
                let drawing: Option<Retained<AnyObject>> =
                    msg_send![context, graphicsContextWithBitmapImageRep: &*rep];
                let drawing = drawing?;
                let _: () = msg_send![context, saveGraphicsState];
                let _: () = msg_send![context, setCurrentContext: &*drawing];
                let _: () = msg_send![
                    &image,
                    drawInRect: square,
                    fromRect: NSRect::ZERO,
                    operation: 2usize,
                    fraction: 1.0f64
                ];
                let _: () = msg_send![context, restoreGraphicsState];
                let options = NSDictionary::<NSString, AnyObject>::new();
                let png: Option<Retained<NSData>> =
                    msg_send![&rep, representationUsingType: 4usize, properties: &*options];
                Some(png?.to_vec())
            }
        })
    }
}
