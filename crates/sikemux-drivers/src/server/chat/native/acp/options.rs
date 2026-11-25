//! Asking an ACP agent what it offers to pick, through a session nothing is said in.

use super::*;

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(60);

struct Answers(Rpc);

impl Reads for Answers {
    type Message = Value;
    type Nudge = ();

    fn message(&mut self, message: Value) {
        if let Some(Incoming::Request { id, method, .. }) = self.0.read(message) {
            self.0
                .refuse(id, &format!("Sikemux does not handle {method}"));
        }
    }

    fn closed(&mut self) {
        self.0.close();
    }
}

/// The models and efforts `launch`'s agent offers on its account.
pub async fn read_options(launch: &ChatLaunch) -> Result<Value, String> {
    tokio::time::timeout(LOOKUP_TIMEOUT, read(launch))
        .await
        .map_err(|_| "It took too long to answer".to_owned())?
}

async fn read(launch: &ChatLaunch) -> Result<Value, String> {
    let account = account_of(launch)?;
    let (process, writer, lines) = spawn(launch, account.as_ref())?;
    let rpc = Rpc::new(writer, true);
    let (_reading, _) = Reading::start(Answers(rpc.clone()), lines);
    let answer = async {
        call(&rpc, AGENT_METHOD_NAMES.initialize, initialize(), None).await?;
        let session = NewSessionRequest::new(&launch.cwd);
        call(&rpc, AGENT_METHOD_NAMES.session_new, session, None).await
    };
    let setup = answer.await.map_err(|error| process.start_error(error));
    let confirmed = match account {
        Some(account) if setup.is_ok() => account.confirmed().await,
        _ => true,
    };
    process.stop().await;
    if !confirmed {
        return Err(accounts::WRONG_STORE.to_owned());
    }
    Ok(native::with_model_config(setup?)
        .get_mut("configOptions")
        .map_or_else(|| json!([]), Value::take))
}
