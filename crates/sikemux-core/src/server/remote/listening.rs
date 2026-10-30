//! Listening for devices: the endpoint, its relays, and the network the accounts server describes.

use super::*;

/// Loads the core's key and devices, listens if remote access was left on,
/// and connects to the account the host is signed in to.
pub(crate) async fn start(core: &Arc<Core>, config: &ServerConfig) {
    let direct_only = config.remote_direct_only;
    let path = file_path(&config.socket);
    let loading = path.clone();
    let (socket, secret_store) = (config.socket.clone(), config.secret_store);
    let loaded = blocking(move || {
        let keeper = Keeper::new(&socket, secret_store)?;
        let (stored, key, trusted) = load(&keeper, &loading)?;
        Ok((stored, key, trusted, keeper))
    })
    .await;
    let (stored, key, trusted, keeper) = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!(
                "sikemux core: remote access is unavailable, {} did not load: {error}",
                path.display()
            );
            return;
        }
    };
    let enabled = stored.enabled;
    {
        let mut inner = core.remote.inner.locked();
        inner.relays = stored
            .network
            .as_ref()
            .and_then(network::usable_relays)
            .unwrap_or_else(network::default_relays);
        inner.path = Some(path);
        inner.keeper = Some(keeper);
        inner.bound = key.account.clone();
        inner.kept = (trusted.clone(), key.account);
        inner.written = trusted;
        inner.direct_only = direct_only;
        inner.never_too_old = cfg!(debug_assertions);
        inner.secret = Some(key.secret);
        inner.retired = key.retired;
        inner.stored = stored;
        inner.accounts_api = config.accounts_api.clone();
    }
    ensure_live(core);
    leave_with_retired_key(core);
    crate::server::share::restore(core);
    if enabled {
        if let Err(error) = listen(core).await {
            eprintln!("sikemux core: remote access did not start: {error}");
        }
    }
    watch_network(core);
}

/// Reads the network now and every few hours while the core runs. Until the
/// first answer, the host uses the last copy it saved.
pub(super) fn watch_network(core: &Arc<Core>) {
    let mut inner = core.remote.inner.locked();
    let Some(base) = inner.accounts_api.clone() else {
        return;
    };
    if inner.direct_only || inner.network.is_some() {
        return;
    }
    let refresh = core.remote.refresh.clone();
    let core = Arc::downgrade(core);
    inner.network = Some(tokio::spawn(async move {
        loop {
            let fetched = network::fetch(&base).await;
            let Some(core) = core.upgrade() else {
                return;
            };
            let wait = match fetched {
                Some(fetched) => {
                    apply_network(&core, fetched).await;
                    NETWORK_REFRESH
                }
                None => NETWORK_RETRY,
            };
            drop(core);
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = refresh.notified() => {}
            }
        }
    }));
}

/// The network came back or changed, or the computer woke: everything that waits
/// on the network tries again now rather than sitting out its backoff.
pub(crate) async fn network_back(core: &Arc<Core>) {
    let endpoint = core
        .remote
        .inner
        .locked()
        .running
        .as_ref()
        .map(|running| running.endpoint.clone());
    if let Some(endpoint) = endpoint {
        endpoint.network_change().await;
    }
    core.remote.refresh.notify_one();
    ensure_live(core);
    crate::server::share::network_back(core);
}

pub(crate) fn stop_network(core: &Core) {
    if let Some(task) = core.remote.inner.locked().network.take() {
        task.abort();
    }
}

/// Moves a listening endpoint onto the network's relays, and turns remote
/// access and the account off while this build is older than the server
/// allows.
pub(super) async fn apply_network(core: &Arc<Core>, fetched: Network) {
    let relays = network::usable_relays(&fetched);
    let version = &core.build.version;
    let (moved, endpoint, required, was_required) = {
        let mut inner = core.remote.inner.locked();
        let required = if inner.never_too_old {
            None
        } else {
            network::too_old(
                version,
                crate::server::host::minimum_version(&fetched.minimum_versions),
            )
            .map(|minimum| UpdateRequired {
                current: version.clone(),
                minimum,
            })
        };
        let moved = match &relays {
            Some(relays) if *relays != inner.relays => {
                let changes = network::relay_changes(&inner.relays, relays);
                inner.relays = relays.clone();
                Some(changes)
            }
            _ => None,
        };
        let was_required = std::mem::replace(&mut inner.update_required, required.clone());
        let endpoint = inner
            .running
            .as_ref()
            .map(|running| running.endpoint.clone());
        (moved, endpoint, required, was_required)
    };
    if relays.is_some() {
        let saved = core.remote.change(|stored| {
            if stored.network.as_ref() != Some(&fetched) {
                stored.network = Some(fetched);
            }
            Ok(())
        });
        if let Err(error) = saved {
            eprintln!("sikemux core: could not save the network: {error}");
        }
    }
    if let (Some((removed, added)), Some(endpoint)) = (moved, endpoint) {
        for config in added {
            endpoint.insert_relay(config.url.clone(), config).await;
        }
        for url in &removed {
            endpoint.remove_relay(url).await;
        }
    }
    match (&was_required, &required) {
        (None, Some(required)) => {
            eprintln!(
                "sikemux core: this build ({}) is older than {}, the oldest the accounts server works with; remote access and the account are off until Sikemux updates",
                required.current, required.minimum
            );
            stop(core).await;
            stop_live(core);
            core.updates.check_soon();
        }
        (Some(_), None) => {
            ensure_live(core);
            if core.remote.is_enabled() {
                if let Err(error) = listen(core).await {
                    eprintln!("sikemux core: remote access did not start: {error}");
                }
            }
        }
        _ => {}
    }
    if was_required != required {
        announce(core);
    }
}

pub(super) fn update_first(required: &UpdateRequired) -> CoreError {
    CoreError::from(format!(
        "update Sikemux first: this version ({}) is older than {}, the oldest the accounts server works with",
        required.current, required.minimum
    ))
}

pub(super) async fn listen(core: &Arc<Core>) -> CoreResult<()> {
    let (secret, direct_only, relays) = {
        let inner = core.remote.inner.locked();
        if let Some(required) = &inner.update_required {
            return Err(update_first(required));
        }
        if inner.running.is_some() {
            return Ok(());
        }
        let secret = inner
            .secret
            .clone()
            .ok_or_else(|| CoreError::from("remote access has no key"))?;
        (secret, inner.direct_only, inner.relays.clone())
    };
    let builder = if direct_only {
        Endpoint::builder(presets::Minimal)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .map_err(|error| CoreError::from(error.to_string()))?
    } else {
        Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Custom(network::relay_map(&relays)))
    };
    let endpoint = builder
        .secret_key(secret)
        .alpns(vec![
            CORE_ALPN.to_vec(),
            JOIN_ALPN.to_vec(),
            crate::server::hosts::LINK_ALPN.to_vec(),
        ])
        .bind()
        .await
        .map_err(|error| CoreError::from(format!("remote access did not start: {error}")))?;
    let accept = tokio::spawn(accept(core.clone(), endpoint.clone()));
    let mut inner = core.remote.inner.locked();
    if inner.running.is_some() {
        accept.abort();
        return Ok(());
    }
    inner.running = Some(Running { endpoint, accept });
    drop(inner);
    core.hosts.wake.now();
    Ok(())
}

/// Stops listening and drops every device connection, handing back the
/// endpoint for the caller to close.
pub(super) fn halt(core: &Core) -> Option<Endpoint> {
    let running = {
        let mut inner = core.remote.inner.locked();
        inner.pending.clear();
        inner.running.take()
    };
    core.close_device_clients(None);
    crate::server::hosts::halt(core);
    running.map(|running| {
        running.accept.abort();
        running.endpoint
    })
}

pub(crate) async fn stop(core: &Core) {
    if let Some(endpoint) = halt(core) {
        endpoint.close().await;
    }
}

pub(crate) async fn set_enabled(core: &Arc<Core>, enabled: bool) -> CoreResult<RemoteStatus> {
    if enabled && core.remote.inner.locked().stored.owner.is_none() {
        return Err(SIGNED_OUT.into());
    }
    if let Some(required) = core
        .remote
        .inner
        .locked()
        .update_required
        .as_ref()
        .filter(|_| enabled)
    {
        return Err(update_first(required));
    }
    core.remote.change(|stored| {
        stored.enabled = enabled;
        Ok(())
    })?;
    if enabled {
        if let Err(error) = listen(core).await {
            core.remote.change(|stored| {
                stored.enabled = false;
                Ok(())
            })?;
            return Err(error);
        }
    } else {
        stop(core).await;
    }
    Ok(announce(core))
}

/// Boxed because a client it serves can turn remote access on, which starts
/// this loop.
pub(super) fn accept(
    core: Arc<Core>,
    endpoint: Endpoint,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async move {
        while let Some(incoming) = endpoint.accept().await {
            tokio::spawn(serve_device(core.clone(), incoming));
        }
    })
}

pub(super) async fn serve_device(core: Arc<Core>, incoming: Incoming) {
    let Ok(connection) = incoming.await else {
        return;
    };
    if connection.alpn() == JOIN_ALPN {
        crate::server::join::serve(core, connection).await;
        return;
    }
    if connection.alpn() == crate::server::hosts::LINK_ALPN {
        crate::server::hosts::serve(core, connection).await;
        return;
    }
    let id = connection.remote_id().to_string();
    if core.remote.standing_of(&id).is_none() {
        connection.close(
            crate::remote::NOT_PAIRED.into(),
            b"this device is not paired with this core",
        );
        return;
    }
    let Ok((send, recv)) = connection.accept_bi().await else {
        return;
    };
    core.remote.note_connected(&id, true);
    announce(&core);
    serve_client(
        core.clone(),
        Peer::Device { id: id.clone() },
        None,
        Box::new(recv),
        Box::new(send),
    )
    .await;
    core.remote.note_connected(&id, false);
    announce(&core);
    connection.close(0u32.into(), b"");
}

impl Remote {
    /// The endpoint remote access listens on, while it does.
    pub(crate) fn endpoint(&self) -> Option<Endpoint> {
        self.inner
            .locked()
            .running
            .as_ref()
            .map(|running| running.endpoint.clone())
    }

    /// The relays this host listens on, which a linked host listens on too.
    /// None while it listens only on loopback.
    pub(crate) fn relays(&self) -> Vec<Relay> {
        let inner = self.inner.locked();
        if inner.direct_only {
            return Vec::new();
        }
        inner.relays.clone()
    }

    #[cfg(test)]
    pub(crate) fn listen_on_loopback(&self) {
        self.inner.locked().direct_only = true;
    }

    /// Notes what the server did with a push. True when the phone's state
    /// changed, so the app should hear.
    #[cfg(test)]
    pub(crate) fn set_accounts_api(&self, base: &str) {
        self.inner.locked().accounts_api = Some(base.into());
    }
}
