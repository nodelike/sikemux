//! Android gives a library its Java VM once, when it loads the library. Rust
//! code cannot find the app's context on its own, and three of iroh's parts
//! need it: its DNS resolver, its network watcher and the certificate check
//! behind its relays. Without it they panic the first time they run. Their
//! warnings go to logcat under the tag `sikemux`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use jni::sys::{jint, JNI_VERSION_1_6};
use jni::{jni_sig, jni_str, Env, JavaVM};

static VM: OnceLock<JavaVM> = OnceLock::new();
static HANDED_OVER: AtomicBool = AtomicBool::new(false);

/// Answers whether the app's context could be handed over yet: Android has
/// none while the library loads before the app finishes starting.
fn hand_over(env: &mut Env) -> jni::errors::Result<bool> {
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
    let context = env.new_global_ref(&app)?;
    // SAFETY: both pointers stay valid for the life of the process: the VM is
    // the one running this app, and the global reference is never dropped.
    unsafe {
        ndk_context::initialize_android_context(
            env.get_java_vm()?.get_raw().cast(),
            context.as_raw().cast(),
        );
    }
    std::mem::forget(context);
    rustls_platform_verifier::android::init_with_env(env, app)?;
    Ok(true)
}

fn try_hand_over(vm: &JavaVM) -> Result<(), String> {
    match vm.attach_current_thread(hand_over) {
        Ok(true) => {
            HANDED_OVER.store(true, Ordering::Release);
            Ok(())
        }
        Ok(false) => Err("Android has not finished starting the app".into()),
        Err(error) => Err(format!("the app's context is out of reach: {error}")),
    }
}

/// iroh panics without the app's context, so the phone stays offline until
/// it has been handed over.
pub(crate) fn ensure_context() -> Result<(), String> {
    if HANDED_OVER.load(Ordering::Acquire) {
        return Ok(());
    }
    let vm = VM.get().ok_or("Android never loaded the network library")?;
    try_hand_over(vm)
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
    if let Err(error) = try_hand_over(&vm) {
        tracing::warn!("{error}; trying again when the phone comes online");
    }
    let _ = VM.set(vm);
    JNI_VERSION_1_6
}
