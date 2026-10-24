/// This device's battery, as its power sources last reported.
#[derive(Clone, Copy, PartialEq)]
pub struct Battery {
    pub percent: u8,
    /// On mains power, full or not, as the Mac has always counted it.
    pub charging: bool,
    /// Until full while charging, else until empty, once the system has worked it out.
    pub minutes: Option<u32>,
}

#[cfg(target_os = "macos")]
pub use mac::{read, watch};

#[cfg(not(target_os = "macos"))]
pub fn read() -> Option<Battery> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn watch(_changed: impl Fn() + 'static) {}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::ffi::c_void;

    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFType, CFTypeRef, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::runloop::{
        kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopSource, CFRunLoopSourceRef,
    };
    use core_foundation::string::CFString;

    use super::Battery;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
        fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFArrayRef;
        fn IOPSGetPowerSourceDescription(blob: CFTypeRef, source: CFTypeRef) -> CFDictionaryRef;
        fn IOPSNotificationCreateRunLoopSource(
            callback: extern "C" fn(*mut c_void),
            context: *mut c_void,
        ) -> CFRunLoopSourceRef;
    }

    thread_local! {
        static CHANGED: RefCell<Option<Box<dyn Fn()>>> = RefCell::new(None);
    }

    pub fn read() -> Option<Battery> {
        // SAFETY: each copy is owned by the wrapper made under the create rule, and the
        // descriptions it hands out live as long as the blob, which outlives them here.
        unsafe {
            let blob = IOPSCopyPowerSourcesInfo();
            if blob.is_null() {
                return None;
            }
            let blob = CFType::wrap_under_create_rule(blob);
            let list = IOPSCopyPowerSourcesList(blob.as_CFTypeRef());
            if list.is_null() {
                return None;
            }
            let list: CFArray<CFType> = CFArray::wrap_under_create_rule(list);
            list.iter().find_map(|source| {
                let description =
                    IOPSGetPowerSourceDescription(blob.as_CFTypeRef(), source.as_CFTypeRef());
                if description.is_null() {
                    return None;
                }
                let description: CFDictionary<CFString, CFType> =
                    CFDictionary::wrap_under_get_rule(description);
                let text = |key: &'static str| {
                    description
                        .find(CFString::from_static_string(key))
                        .and_then(|value| value.downcast::<CFString>())
                        .map(|text| text.to_string())
                };
                let number = |key: &'static str| {
                    description
                        .find(CFString::from_static_string(key))
                        .and_then(|value| value.downcast::<CFNumber>())
                        .and_then(|number| number.to_i64())
                };
                if text("Type").as_deref() != Some("InternalBattery") {
                    return None;
                }
                let charging = text("Power Source State").as_deref() == Some("AC Power");
                let percent = number("Current Capacity")? * 100 / number("Max Capacity")?.max(1);
                let minutes = number(if charging {
                    "Time to Full Charge"
                } else {
                    "Time to Empty"
                })
                .filter(|minutes| *minutes > 0);
                Some(Battery {
                    percent: percent.clamp(0, 100) as u8,
                    charging,
                    minutes: minutes.map(|minutes| minutes as u32),
                })
            })
        }
    }

    extern "C" fn changed(_context: *mut c_void) {
        CHANGED.with(|changed| {
            if let Some(changed) = changed.borrow().as_ref() {
                changed();
            }
        });
    }

    /// Calls `changed` on this thread, which must be the main one, whenever a power source changes.
    pub fn watch(changed_now: impl Fn() + 'static) {
        CHANGED.with(|changed| *changed.borrow_mut() = Some(Box::new(changed_now)));
        // SAFETY: the source is owned by its wrapper; the main run loop keeps its own reference.
        unsafe {
            let source = IOPSNotificationCreateRunLoopSource(changed, std::ptr::null_mut());
            if source.is_null() {
                return;
            }
            let source = CFRunLoopSource::wrap_under_create_rule(source);
            CFRunLoop::get_main().add_source(&source, kCFRunLoopDefaultMode);
        }
    }
}
