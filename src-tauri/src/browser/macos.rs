//! The parts of a browser tab only AppKit can do: page dialogs as window
//! sheets, history without a script round trip, the address of a page that
//! moved without loading anything, and app shortcuts pressed while the page
//! owns the keyboard. Everything here runs on the main thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSBitmapImageFileType,
    NSBitmapImageRep, NSEvent, NSEventMask, NSEventModifierFlags, NSImage,
    NSImageCompressionFactor, NSModalResponse, NSTextField, NSView,
};
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSNumber,
    NSObject, NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString,
};
use objc2_web_kit::{
    WKContentWorld, WKFrameInfo, WKMediaCaptureType, WKNavigationAction, WKOpenPanelParameters,
    WKPDFConfiguration, WKPermissionDecision, WKSecurityOrigin, WKSnapshotConfiguration,
    WKUIDelegate, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};
use tauri::{AppHandle, Emitter};

use super::{BrowserShortcut, PageDialog, BROWSER_SHORTCUT_EVENT};

/// The property the tab watches to hear about a page that moved on its own.
const URL_KEY_PATH: &str = "URL";

struct NativeTab {
    agent_id: String,
    webview: Retained<WKWebView>,
    _delegate: Retained<TabUiDelegate>,
    address_observer: Retained<AddressObserver>,
}

/* AppKit throws if a view is freed while anything is still watching it, so the
tab lets go of the address before it lets go of either of them. */
impl Drop for NativeTab {
    fn drop(&mut self) {
        unsafe {
            self.webview.removeObserver_forKeyPath(
                &self.address_observer,
                &NSString::from_str(URL_KEY_PATH),
            );
        }
    }
}

thread_local! {
    static TABS: RefCell<HashMap<String, NativeTab>> = RefCell::new(HashMap::new());
    static SHORTCUT_MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static OPEN_DIALOGS: RefCell<HashMap<String, OpenDialog>> = RefCell::new(HashMap::new());
}

/// A page dialog showing as a sheet, kept so the agent can answer it too.
struct OpenDialog {
    alert: Retained<NSAlert>,
    field: Option<Retained<NSTextField>>,
}

fn webview_from(pointer: *mut c_void) -> Option<Retained<WKWebView>> {
    unsafe { Retained::retain(pointer.cast::<WKWebView>()) }
}

/// Take over the tab's UI delegate so page dialogs get a sheet, watch where the
/// page says it is, and remember the view so shortcuts can tell which tab has
/// focus. `moved` hears the new address and whether history can go either way;
/// `dialog` hears a page dialog open and close; `upload` hands over files the
/// agent picked for the next file chooser, which then never shows.
pub fn adopt(
    pointer: *mut c_void,
    agent_id: String,
    tab_id: String,
    moved: impl Fn(String, bool, bool) + 'static,
    dialog: impl Fn(Option<PageDialog>) + 'static,
    upload: impl Fn() -> Option<Vec<std::path::PathBuf>> + 'static,
) {
    let (Some(webview), Some(mtm)) = (webview_from(pointer), MainThreadMarker::new()) else {
        return;
    };
    let inner = unsafe { webview.UIDelegate() };
    let delegate = TabUiDelegate::new(
        mtm,
        inner,
        tab_id.clone(),
        Rc::new(dialog),
        Box::new(upload),
    );
    let address_observer = AddressObserver::new(mtm, Box::new(moved));
    unsafe {
        webview.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        webview.setAllowsBackForwardNavigationGestures(true);
        webview.setAllowsMagnification(true);
        webview.addObserver_forKeyPath_options_context(
            &address_observer,
            &NSString::from_str(URL_KEY_PATH),
            NSKeyValueObservingOptions::empty(),
            std::ptr::null_mut(),
        );
    }
    TABS.with(|tabs| {
        tabs.borrow_mut().insert(
            tab_id,
            NativeTab {
                agent_id,
                webview,
                _delegate: delegate,
                address_observer,
            },
        );
    });
}

pub fn forget(tab_id: &str) {
    let _ = answer_dialog(tab_id, false, None);
    TABS.with(|tabs| {
        tabs.borrow_mut().remove(tab_id);
    });
}

struct AddressObserverIvars {
    moved: Box<dyn Fn(String, bool, bool)>,
}

define_class!(
    /// What tells the app that a page changed its address without loading a new
    /// document — a web app routing between its own screens, or a jump to an
    /// anchor. The navigation hooks never hear about either one.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = AddressObserverIvars]
    struct AddressObserver;

    unsafe impl NSObjectProtocol for AddressObserver {}

    impl AddressObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        unsafe fn address_changed(
            &self,
            _key_path: Option<&NSString>,
            object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            let Some(webview) = object.and_then(|object| object.downcast_ref::<WKWebView>()) else {
                return;
            };
            /* A page that is fetching a document reports that address itself
               when the load commits, and may yet be sent somewhere else or fail
               outright. Leaving those to the navigation hook keeps the bar from
               ever showing a page that never arrived. */
            if unsafe { webview.isLoading() } {
                return;
            }
            let Some(address) = (unsafe { webview.URL() }) else {
                return;
            };
            let Some(address) = address.absoluteString() else {
                return;
            };
            (self.ivars().moved)(address.to_string(), unsafe { webview.canGoBack() }, unsafe {
                webview.canGoForward()
            });
        }
    }
);

impl AddressObserver {
    fn new(mtm: MainThreadMarker, moved: Box<dyn Fn(String, bool, bool)>) -> Retained<Self> {
        let observer = mtm
            .alloc::<AddressObserver>()
            .set_ivars(AddressObserverIvars { moved });
        unsafe { msg_send![super(observer), init] }
    }
}

pub fn history(pointer: *mut c_void, delta: i32) {
    let Some(webview) = webview_from(pointer) else {
        return;
    };
    unsafe {
        if delta < 0 {
            webview.goBack();
        } else {
            webview.goForward();
        }
    }
}

pub fn history_state(pointer: *mut c_void) -> (bool, bool) {
    webview_from(pointer)
        .map(|webview| unsafe { (webview.canGoBack(), webview.canGoForward()) })
        .unwrap_or((false, false))
}

/// The visible page as a JPEG at 1x: plenty for a model to read at a fraction
/// of the bytes of a Retina PNG.
pub fn snapshot_jpeg(pointer: *mut c_void, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
    let (Some(webview), Some(mtm)) = (webview_from(pointer), MainThreadMarker::new()) else {
        done(Err("the tab is gone".into()));
        return;
    };
    let configuration = unsafe { WKSnapshotConfiguration::new(mtm) };
    let scale = webview
        .window()
        .map(|window| window.backingScaleFactor())
        .unwrap_or(1.0)
        .max(1.0);
    let width = (webview.frame().size.width / scale).max(1.0);
    unsafe {
        configuration.setSnapshotWidth(Some(&NSNumber::numberWithDouble(width)));
        configuration.setAfterScreenUpdates(true);
    }
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        let Some(done) = done.lock().ok().and_then(|mut slot| slot.take()) else {
            return;
        };
        let image = unsafe { Retained::retain(image) };
        let Some(image) = image else {
            done(Err(unsafe { Retained::retain(error) }
                .map(|error| error.localizedDescription().to_string())
                .unwrap_or_else(|| "the page could not be captured".into())));
            return;
        };
        let Some(pixels) = image.TIFFRepresentation().map(|tiff| tiff.to_vec()) else {
            done(Err("could not encode the page image".into()));
            return;
        };
        // Compressing the picture takes milliseconds, and this block runs on
        // the thread that draws every window.
        std::thread::spawn(move || {
            done(jpeg_bytes(&pixels).ok_or_else(|| "could not encode the page image".to_string()));
        });
    });
    unsafe {
        webview.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &block)
    };
}

/// Runs `body` as the body of an async function in the page, awaiting any
/// promise it returns. The body must return a string.
pub fn call_async(
    pointer: *mut c_void,
    body: &str,
    done: Box<dyn FnOnce(Result<String, String>) + Send>,
) {
    let (Some(webview), Some(mtm)) = (webview_from(pointer), MainThreadMarker::new()) else {
        done(Err("the tab is gone".into()));
        return;
    };
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
        let Some(done) = done.lock().ok().and_then(|mut slot| slot.take()) else {
            return;
        };
        if let Some(error) = unsafe { Retained::retain(error) } {
            done(Err(script_error(&error)));
            return;
        }
        let text = unsafe { value.as_ref() }
            .and_then(|value| value.downcast_ref::<NSString>())
            .map(|text| text.to_string());
        done(text.ok_or_else(|| "the script returned nothing readable".into()));
    });
    unsafe {
        webview.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(body),
            None,
            None,
            &WKContentWorld::pageWorld(mtm),
            Some(&block),
        );
    }
}

/// WebKit's own description of a thrown exception is only "A JavaScript
/// exception occurred"; the page's message sits in the error's details.
fn script_error(error: &NSError) -> String {
    let details = error.userInfo();
    let message = details
        .objectForKey(&NSString::from_str("WKJavaScriptExceptionMessage"))
        .and_then(|value| value.downcast::<NSString>().ok())
        .map(|text| text.to_string());
    message.unwrap_or_else(|| error.localizedDescription().to_string())
}

/// The whole page rather than the part on screen, as a JPEG at 1x. WebKit
/// lays it out as one tall PDF page, which is then drawn as a picture. A page
/// taller than `most` is cut at that height.
pub fn full_page_jpeg(
    pointer: *mut c_void,
    height: f64,
    most: f64,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    let (Some(webview), Some(mtm)) = (webview_from(pointer), MainThreadMarker::new()) else {
        done(Err("the tab is gone".into()));
        return;
    };
    let configuration = unsafe { WKPDFConfiguration::new(mtm) };
    if height > most {
        let width = webview.frame().size.width;
        unsafe {
            configuration.setRect(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, most),
            ))
        };
    }
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |data: *mut NSData, error: *mut NSError| {
        let Some(done) = done.lock().ok().and_then(|mut slot| slot.take()) else {
            return;
        };
        let Some(data) = (unsafe { Retained::retain(data) }) else {
            done(Err(unsafe { Retained::retain(error) }
                .map(|error| error.localizedDescription().to_string())
                .unwrap_or_else(|| "the page could not be captured".into())));
            return;
        };
        let pixels = NSImage::initWithData(mtm.alloc::<NSImage>(), &data)
            .and_then(|image| image.TIFFRepresentation())
            .map(|tiff| tiff.to_vec());
        let Some(pixels) = pixels else {
            done(Err("could not draw the page".into()));
            return;
        };
        std::thread::spawn(move || {
            done(jpeg_bytes(&pixels).ok_or_else(|| "could not encode the page image".to_string()));
        });
    });
    unsafe { webview.createPDFWithConfiguration_completionHandler(Some(&configuration), &block) };
}

fn jpeg_bytes(image: &[u8]) -> Option<Vec<u8>> {
    let bitmap = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(image))?;
    let quality = NSNumber::numberWithDouble(0.82);
    let properties: Retained<NSDictionary<NSString, AnyObject>> =
        NSDictionary::from_slices(&[unsafe { NSImageCompressionFactor }], &[&*quality]);
    let jpeg = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::JPEG, &properties)
    }?;
    Some(jpeg.to_vec())
}

struct TabUiDelegateIvars {
    inner: Option<Retained<ProtocolObject<dyn WKUIDelegate>>>,
    tab_id: String,
    dialog: Rc<dyn Fn(Option<PageDialog>)>,
    upload: Box<dyn Fn() -> Option<Vec<std::path::PathBuf>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TabUiDelegateIvars]
    struct TabUiDelegate;

    unsafe impl NSObjectProtocol for TabUiDelegate {}

    unsafe impl WKUIDelegate for TabUiDelegate {
        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        unsafe fn alert(
            &self,
            webview: &WKWebView,
            message: &NSString,
            frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let done = handler.copy();
            present_sheet(
                self.ivars(),
                webview,
                frame,
                &message.to_string(),
                Sheet::Alert,
                move |_, _| {
                    done.call(());
                },
            );
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        unsafe fn confirm(
            &self,
            webview: &WKWebView,
            message: &NSString,
            frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn(Bool)>,
        ) {
            let done = handler.copy();
            present_sheet(
                self.ivars(),
                webview,
                frame,
                &message.to_string(),
                Sheet::Confirm,
                move |accepted, _| {
                    done.call((Bool::new(accepted),));
                },
            );
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        unsafe fn prompt(
            &self,
            webview: &WKWebView,
            prompt: &NSString,
            default_text: Option<&NSString>,
            frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn(*mut NSString)>,
        ) {
            let done = handler.copy();
            let default = default_text
                .map(|text| text.to_string())
                .unwrap_or_default();
            present_sheet(
                self.ivars(),
                webview,
                frame,
                &prompt.to_string(),
                Sheet::Prompt(default),
                move |accepted, text| {
                    if accepted {
                        let answer = NSString::from_str(&text);
                        done.call((Retained::as_ptr(&answer) as *mut NSString,));
                    } else {
                        done.call((std::ptr::null_mut(),));
                    }
                },
            );
        }

        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        unsafe fn media_capture(
            &self,
            _webview: &WKWebView,
            _origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            _kind: WKMediaCaptureType,
            decision: &block2::DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            decision.call((WKPermissionDecision::Grant,));
        }

        #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
        unsafe fn open_panel(
            &self,
            webview: &WKWebView,
            parameters: &WKOpenPanelParameters,
            frame: &WKFrameInfo,
            handler: &block2::DynBlock<
                dyn Fn(*const objc2_foundation::NSArray<objc2_foundation::NSURL>),
            >,
        ) {
            if let Some(mut paths) = (self.ivars().upload)() {
                if !parameters.allowsMultipleSelection() {
                    paths.truncate(1);
                }
                let urls: Vec<Retained<objc2_foundation::NSURL>> = paths
                    .iter()
                    .map(|path| {
                        objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(
                            &path.to_string_lossy(),
                        ))
                    })
                    .collect();
                let chosen = objc2_foundation::NSArray::from_retained_slice(&urls);
                handler.call((Retained::as_ptr(&chosen),));
                return;
            }
            match &self.ivars().inner {
                Some(inner) => {
                    let _: () = msg_send![
                        &**inner,
                        webView: webview,
                        runOpenPanelWithParameters: parameters,
                        initiatedByFrame: frame,
                        completionHandler: handler
                    ];
                }
                None => handler.call((std::ptr::null(),)),
            }
        }

        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        unsafe fn create_web_view(
            &self,
            webview: &WKWebView,
            configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            match self.ivars().inner.as_ref() {
                Some(inner) => msg_send![
                    &**inner,
                    webView: webview,
                    createWebViewWithConfiguration: configuration,
                    forNavigationAction: action,
                    windowFeatures: features
                ],
                None => None,
            }
        }
    }
);

impl TabUiDelegate {
    fn new(
        mtm: MainThreadMarker,
        inner: Option<Retained<ProtocolObject<dyn WKUIDelegate>>>,
        tab_id: String,
        dialog: Rc<dyn Fn(Option<PageDialog>)>,
        upload: Box<dyn Fn() -> Option<Vec<std::path::PathBuf>>>,
    ) -> Retained<Self> {
        let delegate = mtm.alloc::<TabUiDelegate>().set_ivars(TabUiDelegateIvars {
            inner,
            tab_id,
            dialog,
            upload,
        });
        unsafe { msg_send![super(delegate), init] }
    }
}

enum Sheet {
    Alert,
    Confirm,
    Prompt(String),
}

/// A page dialog as a sheet on the app window, the way Safari shows them. The
/// page waits on `answer`, so every path must call it exactly once. The agent
/// answers through `answer_dialog`, which ends the same sheet.
fn present_sheet(
    tab: &TabUiDelegateIvars,
    webview: &WKWebView,
    frame: &WKFrameInfo,
    message: &str,
    sheet: Sheet,
    answer: impl Fn(bool, String) + 'static,
) {
    let (Some(window), Some(mtm)) = (webview.window(), MainThreadMarker::new()) else {
        answer(false, String::new());
        return;
    };
    let host = unsafe { frame.securityOrigin().host().to_string() };
    let alert = NSAlert::new(mtm);
    let title = if host.is_empty() {
        "This page says".to_owned()
    } else {
        format!("{host} says")
    };
    alert.setMessageText(&NSString::from_str(&title));
    alert.setInformativeText(&NSString::from_str(message));
    alert.addButtonWithTitle(&NSString::from_str("OK"));
    if !matches!(sheet, Sheet::Alert) {
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    }
    let field = match &sheet {
        Sheet::Prompt(default) => {
            let field = NSTextField::initWithFrame(
                mtm.alloc::<NSTextField>(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(280.0, 24.0)),
            );
            field.setStringValue(&NSString::from_str(default));
            alert.setAccessoryView(Some(&field));
            let _ = window.makeFirstResponder(Some(&field));
            Some(field)
        }
        _ => None,
    };
    let described = PageDialog {
        kind: match &sheet {
            Sheet::Alert => "alert",
            Sheet::Confirm => "confirm",
            Sheet::Prompt(_) => "prompt",
        },
        message: message.to_owned(),
        default_text: match &sheet {
            Sheet::Prompt(default) => Some(default.clone()),
            _ => None,
        },
    };
    OPEN_DIALOGS.with(|open| {
        open.borrow_mut().insert(
            tab.tab_id.clone(),
            OpenDialog {
                alert: alert.clone(),
                field: field.clone(),
            },
        );
    });
    (tab.dialog)(Some(described));
    let (tab_id, closed) = (tab.tab_id.clone(), tab.dialog.clone());
    let block = RcBlock::new(move |response: NSModalResponse| {
        OPEN_DIALOGS.with(|open| open.borrow_mut().remove(&tab_id));
        closed(None);
        let accepted = response == NSAlertFirstButtonReturn;
        let text = field
            .as_ref()
            .map(|field| field.stringValue().to_string())
            .unwrap_or_default();
        answer(accepted, text);
    });
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&block));
}

/// Ends the tab's open dialog as if the person pressed OK or Cancel, typing
/// `text` into a prompt first.
pub fn answer_dialog(tab_id: &str, accept: bool, text: Option<&str>) -> Result<(), String> {
    let (alert, field) = OPEN_DIALOGS
        .with(|open| {
            open.borrow()
                .get(tab_id)
                .map(|dialog| (dialog.alert.clone(), dialog.field.clone()))
        })
        .ok_or("this tab has no open dialog")?;
    if let (Some(field), Some(text)) = (field, text) {
        field.setStringValue(&NSString::from_str(text));
    }
    let sheet = alert.window();
    let parent = sheet.sheetParent().ok_or("the dialog is not showing")?;
    parent.endSheet_returnCode(
        &sheet,
        if accept {
            NSAlertFirstButtonReturn
        } else {
            NSAlertSecondButtonReturn
        },
    );
    Ok(())
}

/// Command chords are the app's, not the page's, apart from the editing set
/// every text field expects to keep.
fn forwards_chord(key: &str, flags: NSEventModifierFlags) -> bool {
    if flags.contains(NSEventModifierFlags::Control) || flags.contains(NSEventModifierFlags::Option)
    {
        return false;
    }
    let mut chars = key.chars();
    let (Some(char), None) = (chars.next(), chars.next()) else {
        return false;
    };
    if !char.is_ascii_graphic() {
        return false;
    }
    !matches!(char.to_ascii_lowercase(), 'a' | 'c' | 'v' | 'x' | 'z' | 'y')
}

fn dom_code(key_code: u16, key: &str) -> String {
    let named = match key_code {
        36 => "Enter",
        48 => "Tab",
        49 => "Space",
        51 => "Backspace",
        53 => "Escape",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        _ => "",
    };
    if !named.is_empty() {
        return named.into();
    }
    let Some(char) = key.chars().next() else {
        return String::new();
    };
    match char {
        'a'..='z' | 'A'..='Z' => format!("Key{}", char.to_ascii_uppercase()),
        '0'..='9' => format!("Digit{char}"),
        '[' => "BracketLeft".into(),
        ']' => "BracketRight".into(),
        ',' => "Comma".into(),
        '.' => "Period".into(),
        '/' => "Slash".into(),
        ';' => "Semicolon".into(),
        '\'' => "Quote".into(),
        '-' => "Minus".into(),
        '=' => "Equal".into(),
        '`' => "Backquote".into(),
        '\\' => "Backslash".into(),
        _ => String::new(),
    }
}

/// Tab under the key window's first responder, if any.
fn focused_tab(event: &NSEvent, mtm: MainThreadMarker) -> Option<(String, String)> {
    let window = event.window(mtm)?;
    let responder = window.firstResponder()?;
    let view = responder.downcast_ref::<NSView>()?;
    TABS.with(|tabs| {
        tabs.borrow()
            .iter()
            .find(|(_, tab)| view.isDescendantOf(&tab.webview))
            .map(|(id, tab)| (id.clone(), tab.agent_id.clone()))
    })
}

pub fn install_shortcuts(app: AppHandle) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        let pass = event.as_ptr();
        let event = unsafe { event.as_ref() };
        let flags = event.modifierFlags();
        if !flags.contains(NSEventModifierFlags::Command) {
            return pass;
        }
        let key = event
            .charactersIgnoringModifiers()
            .map(|chars| chars.to_string())
            .unwrap_or_default();
        if !forwards_chord(&key, flags) {
            return pass;
        }
        let Some((tab_id, agent_id)) = focused_tab(event, mtm) else {
            return pass;
        };
        let _ = app.emit(
            BROWSER_SHORTCUT_EVENT,
            BrowserShortcut {
                agent_id,
                tab_id,
                code: dom_code(event.keyCode(), &key),
                key,
                shift: flags.contains(NSEventModifierFlags::Shift),
                alt: false,
            },
        );
        std::ptr::null_mut()
    });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block)
    };
    SHORTCUT_MONITOR.with(|slot| *slot.borrow_mut() = monitor);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_chords_forward_and_editing_chords_stay_with_the_page() {
        let plain = NSEventModifierFlags::Command;
        assert!(forwards_chord("t", plain));
        assert!(forwards_chord("[", plain));
        assert!(forwards_chord("1", plain));
        assert!(!forwards_chord("c", plain));
        assert!(!forwards_chord("Z", plain | NSEventModifierFlags::Shift));
        assert!(!forwards_chord("t", plain | NSEventModifierFlags::Option));
        assert!(!forwards_chord("", plain));
        assert!(!forwards_chord("\u{F729}", plain));
    }

    #[test]
    fn key_codes_become_dom_codes_the_keymap_matches_on() {
        assert_eq!(dom_code(17, "t"), "KeyT");
        assert_eq!(dom_code(18, "1"), "Digit1");
        assert_eq!(dom_code(33, "["), "BracketLeft");
        assert_eq!(dom_code(36, "\r"), "Enter");
        assert_eq!(dom_code(99, "\u{F704}"), "");
    }

    /// One red pixel, the smallest picture AppKit will decode.
    const PIXEL: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn a_page_image_is_encoded_away_from_the_main_thread() {
        let encoded = std::thread::spawn(|| jpeg_bytes(PIXEL))
            .join()
            .expect("the encoder thread finished")
            .expect("the picture is encoded");
        assert_eq!(&encoded[..2], &[0xff, 0xd8], "that is not a JPEG");
        assert_eq!(
            std::thread::spawn(|| jpeg_bytes(b"not a picture"))
                .join()
                .expect("the encoder thread finished"),
            None
        );
    }
}
