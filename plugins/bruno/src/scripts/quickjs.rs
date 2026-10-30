// QuickJS, built into Linux hosts, which have no JavaScript engine of their own to lend. The
// same contract as JavaScriptCore on macOS: a runtime of its own per run, thrown away after, a
// time limit the engine checks as it runs, and a cap on the memory a script may take.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rquickjs::{Context, Ctx, Function, Runtime};

/// What answers a script's `__send`.
pub type Sender = Box<dyn FnMut(&str) -> String>;

/// The most memory one run may take.
const MEMORY: usize = 256 * 1024 * 1024;

fn failure(ctx: &Ctx<'_>, error: rquickjs::Error) -> String {
    match error {
        rquickjs::Error::Exception => {
            let thrown = ctx.catch();
            thrown
                .as_exception()
                .and_then(rquickjs::Exception::message)
                .or_else(|| thrown.as_string().and_then(|text| text.to_string().ok()))
                .unwrap_or_else(|| "the script threw".to_owned())
        }
        other => other.to_string(),
    }
}

/// Evaluates `source` with `input` as `__input`, `send` answering `__send`, and hands back what
/// it left in `__output`.
pub fn run(source: &str, input: &str, send: Sender, limit: Duration) -> Result<String, String> {
    let runtime = Runtime::new().map_err(|error| error.to_string())?;
    let deadline = Instant::now() + limit;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    runtime.set_memory_limit(MEMORY);
    let context = Context::full(&runtime).map_err(|error| error.to_string())?;
    let send = Rc::new(RefCell::new(send));
    context.with(|ctx| -> Result<(), String> {
        let globals = ctx.globals();
        globals
            .set("__input", input)
            .map_err(|error| failure(&ctx, error))?;
        let answer = Function::new(ctx.clone(), move |asked: String| -> String {
            (send.borrow_mut())(&asked)
        })
        .map_err(|error| failure(&ctx, error))?;
        globals
            .set("__send", answer)
            .map_err(|error| failure(&ctx, error))?;
        ctx.eval::<(), _>(source)
            .map_err(|error| failure(&ctx, error))
    })?;
    // Every job is run, even past the deadline, where each stops at once: QuickJS cannot free
    // a runtime that still holds jobs.
    let mut failed = false;
    while runtime.is_job_pending() {
        if runtime.execute_pending_job().is_err() {
            failed = true;
            context.with(|ctx| drop(ctx.catch()));
        }
    }
    if Instant::now() > deadline {
        return Err("script timed out".to_owned());
    }
    if failed {
        return Err("the script's promises failed".to_owned());
    }
    context.with(|ctx| {
        ctx.globals()
            .get::<_, Option<String>>("__output")
            .map_err(|error| failure(&ctx, error))?
            .ok_or_else(|| "the script did not finish".to_owned())
    })
}
