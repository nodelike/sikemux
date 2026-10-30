//! Android gives a library its Java VM once, when it loads the library. Rust
//! code cannot find the app's context on its own, and three of iroh's parts
//! need it: its DNS resolver, its network watcher and the certificate check
//! behind its relays. Without it they panic the first time they run. Their
//! warnings go to logcat under the tag `sikemux`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use jni::sys::{jint, JNI_VERSION_1_6};
use jni::{jni_sig, jni_str, Env, JavaVM};

static VM: OnceLock<JavaVM> = OnceLock::new();
static HANDED_OVER: AtomicBool = AtomicBool::new(false);
/// Each part is handed over once: Android's context cannot be given twice.
static PARTS: Mutex<Parts> = Mutex::new(Parts {
    context: false,
    verifier: false,
});

struct Parts {
    context: bool,
    verifier: bool,
}

/// Answers whether the app's context could be handed over yet: Android has
/// none while the library loads before the app finishes starting.
fn hand_over(env: &mut Env, parts: &mut Parts) -> jni::errors::Result<bool> {
    let app = env
        .call_static_method(
            jni_str!("android/app/ActivityThread"),
            jni_str!("currentApplication"),
            jni_sig!("()Landroid/app/Application;"),
            &[],
        )?
        .l()?;
    if app.is_null() {
        return Ok(false);
    }
    if !parts.context {
        let context = env.new_global_ref(&app)?;
        // SAFETY: both pointers stay valid for the life of the process: the VM
        // is the one running this app, and the global reference is never
        // dropped.
        unsafe {
            ndk_context::initialize_android_context(
                env.get_java_vm()?.get_raw().cast(),
                context.as_raw().cast(),
            );
        }
        std::mem::forget(context);
        parts.context = true;
    }
    if !parts.verifier {
        rustls_platform_verifier::android::init_with_env(env, app)?;
        parts.verifier = true;
    }
    Ok(true)
}

/// iroh panics without the app's context, so the phone stays offline until
/// it has been handed over.
pub(crate) fn ensure_context() -> Result<(), String> {
    if HANDED_OVER.load(Ordering::Acquire) {
        return Ok(());
    }
    let vm = VM.get().ok_or("Android never loaded the network library")?;
    let mut parts = PARTS.lock().unwrap_or_else(PoisonError::into_inner);
    if HANDED_OVER.load(Ordering::Acquire) {
        return Ok(());
    }
    match vm.attach_current_thread(|env| hand_over(env, &mut parts)) {
        Ok(true) => {
            HANDED_OVER.store(true, Ordering::Release);
            Ok(())
        }
        Ok(false) => Err("Android has not finished starting the app".into()),
        Err(error) => Err(format!("the app's context is out of reach: {error}")),
    }
}

/// Raise the level to see why a connection stalls: iroh traces each path it tries.
fn log_to_logcat() {
    use tracing_subscriber::filter::{LevelFilter, Targets};
    use tracing_subscriber::layer::SubscriberExt;
    let filter = Targets::new().with_default(LevelFilter::WARN);
    let subscriber = tracing_subscriber::registry()
        .with(paranoid_android::layer("sikemux"))
        .with(filter);
    let _ = tracing::subscriber::set_global_default(subscriber);
}

#[unsafe(no_mangle)]
pub extern "system" fn JNI_OnLoad(vm: *mut jni::sys::JavaVM, _reserved: *mut c_void) -> jint {
    // SAFETY: Android passes the VM that is loading this library.
    let vm = unsafe { JavaVM::from_raw(vm) };
    log_to_logcat();
    let _ = VM.set(vm);
    if let Err(error) = ensure_context() {
        tracing::warn!("{error}; trying again when the phone comes online");
    }
    JNI_VERSION_1_6
}
