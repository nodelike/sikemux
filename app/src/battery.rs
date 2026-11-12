use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled,
};

use crate::icons::Icon;
use crate::theme::{Tokens, TEXT_2XS};
use crate::workbench::Workbench;

#[derive(Clone, Copy, PartialEq)]
pub struct Battery {
    pub percent: u8,
    /// On mains power, full or not, as the Mac has always counted it.
    pub charging: bool,
    /// Until full while charging, else until empty, once the system has worked it out.
    pub minutes: Option<u32>,
}

use crate::ui;
#[cfg(target_os = "macos")]
pub use mac::{read, watch};

#[cfg(target_os = "linux")]
pub use linux::{read, watch};

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

pub(crate) fn battery_chip(bench: &Workbench, cx: &App) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let battery = bench.battery.as_ref()?;
    let percent = battery.percent;
    let tint = match percent {
        _ if battery.charging => t.live,
        0..=10 => t.danger,
        11..=20 => t.warn,
        _ => t.accent,
    };
    let time = battery
        .minutes
        .map(|minutes| format!("{}:{:02}", minutes / 60, minutes % 60));
    let about = match (battery.charging, time) {
        (true, Some(time)) => t!(
            "{percent}% · charging · {time} until full",
            percent = percent,
            time = time
        ),
        (true, None) => t!("{percent}% · charging", percent = percent),
        (false, Some(time)) => t!(
            "{percent}% · {time} remaining",
            percent = percent,
            time = time
        ),
        (false, None) => format!("{percent}%"),
    };
    let unit = 12. / 13.;
    let at = |units: f32| px(units * unit);
    let level = f32::from(percent) / 100.;
    let fill = if level > 0. {
        (20.5 * level).max(2.2)
    } else {
        0.
    };
    let frame = Hsla { a: 0.45, ..tint };
    let bolt = |glyph: Icon, color: Hsla| {
        ui::icon(glyph, 19. * unit)
            .w(at(12.))
            .absolute()
            .left(at(7.))
            .top(at(-3.))
            .text_color(color)
    };
    let icon = div()
        .relative()
        .flex_none()
        .w(at(28.))
        .h(at(13.))
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w(at(24.5))
                .h(at(13.))
                .border(at(1.))
                .border_color(frame)
                .rounded(at(4.1)),
        )
        .child(
            div()
                .absolute()
                .left(at(25.4))
                .top(at(4.4))
                .w(at(1.6))
                .h(at(4.2))
                .rounded_r(at(1.6))
                .bg(frame),
        )
        .child(
            div()
                .absolute()
                .left(at(2.))
                .top(at(2.))
                .w(at(fill))
                .h(at(9.))
                .rounded(at(2.2))
                .bg(tint),
        )
        .when(battery.charging, |icon| {
            icon.child(bolt(Icon::BatteryBoltGap, t.surface_sunken))
                .child(bolt(Icon::BatteryBolt, tint))
        });
    Some(
        div()
            .id("battery")
            .flex()
            .items_center()
            .gap(px(5.))
            .tooltip(ui::tooltip(about))
            .child(icon)
            .child(
                ui::text(
                    t!("{percent}%", percent = percent),
                    TEXT_2XS,
                    FontWeight::SEMIBOLD,
                )
                .font_features(crate::theme::tabular())
                .text_color(tint),
            )
            .into_any_element(),
    )
}
