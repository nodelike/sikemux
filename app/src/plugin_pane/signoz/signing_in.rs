//! Signing in to a SigNoz server.

use super::*;

impl Signoz {
    /// The address first, because it answers everything else: which version this is, and,
    /// once an email is typed, whether that person signs in with a password there at all.
    pub(super) fn sign_in(
        &mut self,
        host: &mut Host,
        status: &Status,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        if self.form.is_none() {
            let url_now = status.details.url.clone();
            let email_now = status.details.email.clone();
            let look: Typed = |signoz, host, event, _, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    signoz.inspect(host, true, cx);
                }
            };
            let submit: Typed = |signoz, host, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    signoz.submit(host, cx);
                }
            };
            let mut make = |placeholder: &'static str, masked: bool, typed| {
                let mut slot = None;
                field(
                    &mut slot,
                    &mut self.typing,
                    placeholder,
                    masked,
                    typed,
                    window,
                    cx,
                )
            };
            let url = make("https://signoz.example.com", false, Some(look));
            let email = make("you@example.com", false, Some(look));
            let password = make("Password", true, Some(submit));
            let key = make("API key", true, Some(submit));
            let account = make("Keychain account", false, Some(submit));
            url.update(cx, |input, cx| input.set_value(url_now.clone(), window, cx));
            email.update(cx, |input, cx| input.set_value(email_now, window, cx));
            self.form = Some(SignIn {
                url,
                email,
                password,
                key,
                account,
                method: match status.details.auth {
                    AuthMode::ApiKey => Method::ApiKey,
                    AuthMode::Session => Method::Password,
                },
                keychain: false,
                editing_url: url_now.is_empty(),
                inspection: None,
                org: String::new(),
                busy: false,
                error: None,
            });
            if !url_now.is_empty() {
                self.inspect(host, true, cx);
            }
        }
        let form = self.form.as_ref().expect("made just now");
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let orgs: Vec<Value> = form
            .inspection
            .as_ref()
            .and_then(|found| found["orgs"].as_array().cloned())
            .unwrap_or_default();
        let password_orgs: Vec<&Value> =
            orgs.iter().filter(|org| org["password"] == true).collect();
        let sso_only = form.inspection.is_some() && !orgs.is_empty() && password_orgs.is_empty();
        let ready = !form.busy
            && !value(&form.url).is_empty()
            && match form.method {
                Method::Password => {
                    !value(&form.email).is_empty()
                        && !form.password.read(cx).value().is_empty()
                        && !sso_only
                }
                Method::ApiKey if form.keychain => !value(&form.account).is_empty(),
                Method::ApiKey => !value(&form.key).is_empty(),
            };
        let address = if form.editing_url {
            sign_in::Field::new("SigNoz URL", &form.url)
                .mono()
                .into_any_element()
        } else {
            let version = form
                .inspection
                .as_ref()
                .and_then(|found| found["version"].as_str())
                .map(|version| format!("SigNoz {version}").into());
            let shown = value(&form.url)
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned();
            sign_in::address(
                shown,
                version,
                sign_in::link("signoz-change-url", "Change", cx).on_click(listen::<Signoz, _>(
                    cx,
                    |signoz, _, _, _, cx| {
                        if let Some(form) = &mut signoz.form {
                            form.editing_url = true;
                        }
                        cx.notify();
                    },
                )),
                cx,
            )
            .into_any_element()
        };
        let mut fields = vec![address];
        if status.details.key_from_environment {
            fields.push(
                sign_in::note(
                    "SIGNOZ_API_KEY is set in your shell, so Sikemux uses that key.",
                    cx,
                )
                .into_any_element(),
            );
        }
        match form.method {
            Method::Password => {
                fields.push(sign_in::Field::new("Email", &form.email).into_any_element());
                if form
                    .inspection
                    .as_ref()
                    .is_some_and(|found| found["accountExists"] == false)
                {
                    fields.push(
                        sign_in::note("SigNoz does not know this email.", cx)
                            .w_full()
                            .text_left()
                            .text_color(t.warn)
                            .into_any_element(),
                    );
                }
                if password_orgs.len() > 1 {
                    let picks = password_orgs
                        .iter()
                        .map(|org| {
                            let id = org["id"].as_str().unwrap_or_default().to_owned();
                            let name = org["name"]
                                .as_str()
                                .filter(|name| !name.is_empty())
                                .unwrap_or(&id)
                                .to_owned();
                            ui::Pick::new(id, name)
                        })
                        .collect();
                    let shown = password_orgs
                        .iter()
                        .find(|org| org["id"] == json!(form.org))
                        .and_then(|org| org["name"].as_str())
                        .unwrap_or("Choose one")
                        .to_owned();
                    let pick = ui::select(
                        "signoz-org",
                        shown,
                        picks,
                        Some(form.org.clone().into()),
                        false,
                        listen::<Signoz, SharedString>(cx, |signoz, _, picked, _, cx| {
                            if let Some(form) = &mut signoz.form {
                                form.org = picked.to_string();
                            }
                            cx.notify();
                        }),
                        cx,
                    );
                    fields.push(sign_in::labelled("Organisation", pick, cx).into_any_element());
                }
                if sso_only {
                    let providers: Vec<String> = orgs
                        .iter()
                        .flat_map(|org| org["sso"].as_array().cloned().unwrap_or_default())
                        .filter_map(|sso| sso["provider"].as_str().map(str::to_owned))
                        .collect();
                    fields.push(
                        sign_in::warning(
                            format!(
                                "This organisation signs in through {}. Sikemux cannot do that yet, so use an API key.",
                                providers.join(" or ")
                            ),
                            cx,
                        )
                        .into_any_element(),
                    );
                } else {
                    fields.push(sign_in::Field::new("Password", &form.password).into_any_element());
                }
            }
            Method::ApiKey => {
                fields.push(match form.keychain {
                    true => sign_in::Field::new("Keychain account", &form.account)
                        .mono()
                        .hint("The account your signoz CLI saved its key under, in the signoz-api Keychain item.")
                        .into_any_element(),
                    false => sign_in::Field::new("API key", &form.key)
                        .mono()
                        .hint("In SigNoz: Settings → Service Accounts → a service account → Keys.")
                        .into_any_element(),
                });
                fields.push(
                    div()
                        .w_full()
                        .flex()
                        .child(
                            Toggle::new(
                                "signoz-keychain",
                                "The Keychain already has a key for this SigNoz",
                                form.keychain,
                            )
                            .on_click(listen::<Signoz, _>(
                                cx,
                                |signoz, _, _, _, cx| {
                                    if let Some(form) = &mut signoz.form {
                                        form.keychain = !form.keychain;
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                        .into_any_element(),
                );
            }
        }
        let action = match (form.busy, form.method) {
            (true, _) => "Connecting…",
            (false, Method::Password) => "Sign in",
            (false, Method::ApiKey) => "Use key",
        };
        let other = match form.method {
            Method::Password => "Use an API key instead",
            Method::ApiKey => "Sign in with email instead",
        };
        let settings_page =
            (form.method == Method::ApiKey && !form.keychain && !value(&form.url).is_empty())
                .then(|| format!("{}/settings", value(&form.url).trim_end_matches('/')));
        let foot = (form.method == Method::Password && !sso_only).then_some(
            "Your password goes to SigNoz once. Sikemux keeps only the session, in the macOS Keychain.",
        );
        let error = form.error.clone().or_else(|| {
            status
                .message
                .clone()
                .filter(|_| status.configured)
                .map(Into::into)
        });
        sign_in::Screen::new(
            "signoz-sign-in",
            ui::picture(Icon::Signoz, 28., signoz_mark()),
            "Connect SigNoz",
        )
        .lede("Logs, traces and dashboards from your SigNoz, beside the code that wrote them.")
        .when_some(foot, |screen, foot| screen.foot(foot))
        .children(fields)
        .child(
            Button::new("signoz-submit", action, Variant::Primary)
                .wide()
                .disabled(!ready)
                .on_click(listen::<Signoz, _>(cx, |signoz, host, _, _, cx| {
                    signoz.submit(host, cx)
                })),
        )
        .children(error.map(|error| sign_in::error(error, cx)))
        .child(
            sign_in::links()
                .child(
                    sign_in::link("signoz-method", other, cx).on_click(listen::<Signoz, _>(
                        cx,
                        |signoz, _, _, _, cx| {
                            if let Some(form) = &mut signoz.form {
                                form.method = match form.method {
                                    Method::Password => Method::ApiKey,
                                    Method::ApiKey => Method::Password,
                                };
                            }
                            cx.notify();
                        },
                    )),
                )
                .children(settings_page.map(|page| {
                    sign_in::link("signoz-settings", "Open SigNoz settings", cx)
                        .on_click(move |_, _, cx| cx.open_url(&page))
                })),
        )
        .into_any_element()
    }

    /// Asks the address what SigNoz runs there and, with the email, how that person signs in.
    fn inspect(&mut self, host: &Host, with_email: bool, cx: &mut Context<PluginPane>) {
        let Some(form) = &mut self.form else {
            return;
        };
        let url = form.url.read(cx).value().trim().to_owned();
        if url.is_empty() {
            return;
        }
        let email = form.email.read(cx).value().trim().to_owned();
        let email = (with_email && !email.is_empty()).then_some(email);
        form.error = None;
        after::<Signoz, _>(
            host.call("inspect", json!({ "url": url, "email": email })),
            |signoz, _, found, _| {
                let Some(form) = &mut signoz.form else {
                    return;
                };
                match found {
                    Ok(found) => {
                        let passwords: Vec<String> = found["orgs"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|org| org["password"] == true)
                            .filter_map(|org| org["id"].as_str().map(str::to_owned))
                            .collect();
                        if let [only] = passwords.as_slice() {
                            form.org = only.clone();
                        }
                        form.editing_url = false;
                        form.inspection = Some(found);
                    }
                    Err(failure) => {
                        form.inspection = None;
                        form.editing_url = true;
                        form.error = Some(failure.message);
                    }
                }
            },
            cx,
        );
    }

    fn submit(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(form) = &mut self.form else {
            return;
        };
        if form.busy {
            return;
        }
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let url = value(&form.url);
        let (method, params) = match form.method {
            Method::Password => (
                "signIn",
                json!({
                    "url": url,
                    "email": value(&form.email),
                    "password": form.password.read(cx).value().to_string(),
                    "orgId": (!form.org.is_empty()).then(|| form.org.clone()),
                }),
            ),
            Method::ApiKey if form.keychain => (
                "useApiKey",
                json!({ "url": url, "account": value(&form.account) }),
            ),
            Method::ApiKey => (
                "useApiKey",
                json!({ "url": url, "apiKey": value(&form.key) }),
            ),
        };
        form.busy = true;
        form.error = None;
        after::<Signoz, _>(
            host.call(method, params),
            |signoz, _, done, _| {
                let Some(form) = &mut signoz.form else {
                    return;
                };
                form.busy = false;
                form.error = match done.map(|status| Status::deserialize(&status)) {
                    Ok(Ok(status)) if status.ok => None,
                    Ok(Ok(status)) => Some(
                        status
                            .message
                            .unwrap_or_else(|| "SigNoz did not answer the first query".to_owned())
                            .into(),
                    ),
                    Ok(Err(error)) => Some(error.to_string().into()),
                    Err(failure) => Some(failure.message),
                };
            },
            cx,
        );
        cx.notify();
    }
}
