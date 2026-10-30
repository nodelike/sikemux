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

#[cfg(target_os = "linux")]
pub use linux::{read, watch};

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn read() -> Option<Battery> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn watch(_changed: impl Fn() + Send + 'static) {}

/// UPower's summary of the batteries, which it keeps on the system bus.
#[cfg(target_os = "linux")]
mod linux {
    use std::sync::Mutex;

    use sikemux_process::sync::Locked;
    use zbus::blocking::proxy::Builder;
    use zbus::blocking::{Connection, Proxy};
    use zbus::proxy::CacheProperties;

    use super::Battery;

    const UPOWER: &str = "org.freedesktop.UPower";
    const DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
    const BATTERY_TYPE: u32 = 2;
    const DISCHARGING_STATES: [u32; 3] = [2, 3, 6];

    static LAST: Mutex<Option<Battery>> = Mutex::new(None);

    pub fn read() -> Option<Battery> {
        *LAST.locked()
    }

    /// Calls `changed`, from a thread of its own, whenever the battery reads differently.
    pub fn watch(changed: impl Fn() + Send + 'static) {
        let _ = std::thread::Builder::new()
            .name("battery".into())
            .spawn(move || {
                let _ = follow(changed);
            });
    }

    fn follow(changed: impl Fn()) -> zbus::Result<()> {
        let bus = Connection::system()?;
        let device: Proxy = Builder::new(&bus)
            .destination(UPOWER)?
            .path(DISPLAY_DEVICE)?
            .interface("org.freedesktop.UPower.Device")?
            .cache_properties(CacheProperties::No)
            .build()?;
        let properties = Proxy::new(
            &bus,
            UPOWER,
            DISPLAY_DEVICE,
            "org.freedesktop.DBus.Properties",
        )?;
        let mut changes = properties.receive_signal("PropertiesChanged")?;
        loop {
            let now = battery(&device);
            if std::mem::replace(&mut *LAST.locked(), now) != now {
                changed();
            }
            if changes.next().is_none() {
                return Ok(());
            }
        }
    }

    fn battery(device: &Proxy) -> Option<Battery> {
        let present: bool = device.get_property("IsPresent").ok()?;
        let kind: u32 = device.get_property("Type").ok()?;
        if !present || kind != BATTERY_TYPE {
            return None;
        }
        let percent: f64 = device.get_property("Percentage").ok()?;
        let state: u32 = device.get_property("State").ok()?;
        let charging = !DISCHARGING_STATES.contains(&state);
        let seconds: i64 = device
            .get_property(if charging {
                "TimeToFull"
            } else {
                "TimeToEmpty"
            })
            .unwrap_or(0);
        Some(Battery {
            percent: percent.round().clamp(0., 100.) as u8,
            charging,
            minutes: (seconds > 0).then_some((seconds / 60) as u32),
        })
    }
}

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
