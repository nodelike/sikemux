// macOS's own JavaScriptCore, reached through its C interface, so running scripts adds nothing
// to the app. Each run gets a context of its own, thrown away after, and a time limit the
// engine enforces itself, so a script that loops is stopped rather than left running.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
use std::time::Duration;

type JsContextGroup = *const c_void;
type JsContext = *const c_void;
type JsGlobalContext = *mut c_void;
type JsString = *mut c_void;
type JsValue = *const c_void;
type JsObject = *mut c_void;

type Callback = unsafe extern "C" fn(
    ctx: JsContext,
    function: JsObject,
    this: JsObject,
    count: usize,
    arguments: *const JsValue,
    exception: *mut JsValue,
) -> JsValue;

type ShouldTerminate = unsafe extern "C" fn(ctx: JsContext, context: *mut c_void) -> bool;

#[link(name = "JavaScriptCore", kind = "framework")]
extern "C" {
    fn JSContextGroupCreate() -> JsContextGroup;
    fn JSContextGroupRelease(group: JsContextGroup);
    fn JSGlobalContextCreateInGroup(group: JsContextGroup, class: *const c_void)
        -> JsGlobalContext;
    fn JSGlobalContextRelease(ctx: JsGlobalContext);
    fn JSContextGetGlobalObject(ctx: JsContext) -> JsObject;
    fn JSEvaluateScript(
        ctx: JsContext,
        script: JsString,
        this: JsObject,
        source: JsString,
        line: i32,
        exception: *mut JsValue,
    ) -> JsValue;
    fn JSStringCreateWithCharacters(characters: *const u16, count: usize) -> JsString;
    fn JSStringRelease(string: JsString);
    fn JSStringGetLength(string: JsString) -> usize;
    fn JSStringGetCharactersPtr(string: JsString) -> *const u16;
    fn JSValueToStringCopy(ctx: JsContext, value: JsValue, exception: *mut JsValue) -> JsString;
    fn JSValueMakeString(ctx: JsContext, string: JsString) -> JsValue;
    fn JSValueIsString(ctx: JsContext, value: JsValue) -> bool;
    fn JSObjectMakeFunctionWithCallback(
        ctx: JsContext,
        name: JsString,
        callback: Callback,
    ) -> JsObject;
    fn JSObjectSetProperty(
        ctx: JsContext,
        object: JsObject,
        name: JsString,
        value: JsValue,
        attributes: u32,
        exception: *mut JsValue,
    );
    fn JSObjectGetProperty(
        ctx: JsContext,
        object: JsObject,
        name: JsString,
        exception: *mut JsValue,
    ) -> JsValue;
    // Not in the public headers, but exported and what Safari's own watchdog uses.
    fn JSContextGroupSetExecutionTimeLimit(
        group: JsContextGroup,
        limit: f64,
        callback: Option<ShouldTerminate>,
        context: *mut c_void,
    );
}

/// A JavaScript string made from `text`, released when dropped.
struct Text(JsString);

impl Text {
    fn new(text: &str) -> Self {
        let units: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: the pointer and length describe `units`, which the engine copies.
        Self(unsafe { JSStringCreateWithCharacters(units.as_ptr(), units.len()) })
    }

    /// Takes ownership of a string the engine handed back.
    fn owned(string: JsString) -> Self {
        Self(string)
    }

    fn read(&self) -> String {
        if self.0.is_null() {
            return String::new();
        }
        // SAFETY: the engine owns the characters for as long as the string lives, and `self`
        // keeps it alive while they are copied.
        unsafe {
            let length = JSStringGetLength(self.0);
            let characters = JSStringGetCharactersPtr(self.0);
            if characters.is_null() {
                return String::new();
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(characters, length))
        }
    }
}

impl Drop for Text {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the string was made by or handed over from the engine and not yet released.
            unsafe { JSStringRelease(self.0) }
        }
    }
}

/// What answers a script's `__send`.
pub type Sender = Box<dyn FnMut(&str) -> String>;

thread_local! {
    /// What answers the script's `__send` on this thread, while a run goes on.
    static SEND: RefCell<Option<Sender>> = RefCell::new(None);
}

unsafe extern "C" fn send(
    ctx: JsContext,
    _function: JsObject,
    _this: JsObject,
    count: usize,
    arguments: *const JsValue,
    _exception: *mut JsValue,
) -> JsValue {
    let asked = match count {
        0 => String::new(),
        // SAFETY: the engine passes `count` arguments at `arguments`.
        _ => unsafe {
            let first = *arguments;
            Text::owned(JSValueToStringCopy(ctx, first, ptr::null_mut())).read()
        },
    };
    let answer = SEND.with(|send| match send.borrow_mut().as_mut() {
        Some(send) => send(&asked),
        None => r#"{"error":"sending is not available here"}"#.to_owned(),
    });
    let answer = Text::new(&answer);
    // SAFETY: `ctx` is the context calling us, and the engine copies the string.
    unsafe { JSValueMakeString(ctx, answer.0) }
}

unsafe extern "C" fn stop(_ctx: JsContext, _context: *mut c_void) -> bool {
    true
}

/// Evaluates `source` with `input` as `__input`, `send` answering `__send`, and hands back what
/// it left in `__output`.
pub fn run(
    source: &str,
    input: &str,
    send_with: Sender,
    limit: Duration,
) -> Result<String, String> {
    SEND.with(|send| *send.borrow_mut() = Some(send_with));
    // SAFETY: every value is made in and used with this one context, on this thread, and the
    // context and its group are released before returning.
    let result = unsafe {
        let group = JSContextGroupCreate();
        JSContextGroupSetExecutionTimeLimit(
            group,
            limit.as_secs_f64(),
            Some(stop),
            ptr::null_mut(),
        );
        let ctx = JSGlobalContextCreateInGroup(group, ptr::null());
        let global = JSContextGetGlobalObject(ctx);
        let input_name = Text::new("__input");
        let input_text = Text::new(input);
        JSObjectSetProperty(
            ctx,
            global,
            input_name.0,
            JSValueMakeString(ctx, input_text.0),
            0,
            ptr::null_mut(),
        );
        let send_name = Text::new("__send");
        let function = JSObjectMakeFunctionWithCallback(ctx, send_name.0, send);
        JSObjectSetProperty(
            ctx,
            global,
            send_name.0,
            function as JsValue,
            0,
            ptr::null_mut(),
        );
        let script = Text::new(source);
        let mut exception: JsValue = ptr::null();
        JSEvaluateScript(
            ctx,
            script.0,
            ptr::null_mut(),
            ptr::null_mut(),
            1,
            &mut exception,
        );
        let result = match exception.is_null() {
            false => Err(Text::owned(JSValueToStringCopy(ctx, exception, ptr::null_mut())).read()),
            true => {
                let output_name = Text::new("__output");
                let output = JSObjectGetProperty(ctx, global, output_name.0, ptr::null_mut());
                match !output.is_null() && JSValueIsString(ctx, output) {
                    true => {
                        Ok(Text::owned(JSValueToStringCopy(ctx, output, ptr::null_mut())).read())
                    }
                    false => Err("the script did not finish".to_owned()),
                }
            }
        };
        JSGlobalContextRelease(ctx);
        JSContextGroupRelease(group);
        result
    };
    SEND.with(|send| *send.borrow_mut() = None);
    result
}
