//! Signing in to a Rundeck server.

use super::*;

impl Rundeck {
    pub(super) fn sign_in(
        &mut self,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        if self.sign_in.is_none() {
            let mut make = |placeholder: &str, masked: bool, cx: &mut Context<PluginPane>| {
                let placeholder = placeholder.to_owned();
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .masked(masked)
                });
                self.typing
                    .push(cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()));
                input
            };
            let url = make("https://rundeck.example.com", false, cx);
            let token = make("API token", true, cx);
            let user = make("Username", false, cx);
            let password = make("Password", true, cx);
            self.sign_in = Some(SignIn {
                with_token: true,
                url,
                token,
                user,
                password,
                insecure: false,
                busy: false,
            });
        }
        let message = host
            .status(cx)
            .read::<Status>()
            .ok()
            .flatten()
            .and_then(|status| status.message.clone());
        let form = self.sign_in.as_ref().expect("made just now");
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let filled = !value(&form.url).is_empty()
            && if form.with_token {
                !value(&form.token).is_empty()
            } else {
                !value(&form.user).is_empty() && !value(&form.password).is_empty()
            };
        let mode = Segmented::words(
            "rundeck-sign-in-mode",
            vec![
                ("API token".into(), None),
                ("Username and password".into(), None),
            ],
            usize::from(!form.with_token),
        )
        .on_select(listen::<Rundeck, usize>(cx, |rd, _, index, _, cx| {
            if let Some(form) = &mut rd.sign_in {
                form.with_token = *index == 0;
            }
            cx.notify();
        }));
        let ink = Tokens::get(cx).text_primary;
        sign_in::Screen::new(
            "rundeck-sign-in",
            ui::picture(Icon::Rundeck, 26., ink),
            "Sign in to Rundeck",
        )
        .lede(PITCH)
        .foot(ui::sign_in::KEPT)
        .child(sign_in::Field::new("Rundeck address", &form.url).mono())
        .child(div().w_full().child(mode.fill()))
        .when(form.with_token, |screen| {
            screen.child(sign_in::Field::new("API token", &form.token).mono())
        })
        .when(!form.with_token, |screen| {
            screen
                .child(sign_in::Field::new("Username", &form.user))
                .child(sign_in::Field::new("Password", &form.password))
        })
        .child(
            div().w_full().flex().child(
                Toggle::new(
                    "rundeck-insecure",
                    "Allow plain HTTP on a private network",
                    form.insecure,
                )
                .on_click(listen::<Rundeck, ClickEvent>(cx, |rd, _, _, _, cx| {
                    if let Some(form) = &mut rd.sign_in {
                        form.insecure = !form.insecure;
                    }
                    cx.notify();
                })),
            ),
        )
        .child(
            Button::new(
                "rundeck-sign-in-go",
                if form.busy {
                    "Signing in…"
                } else {
                    "Sign in"
                },
                Variant::Primary,
            )
            .wide()
            .icon(Icon::LogIn)
            .disabled(!filled || form.busy)
            .on_click(listen::<Rundeck, ClickEvent>(cx, |rd, host, _, _, cx| {
                rd.sign_in_now(host, cx)
            })),
        )
        .children(message.map(|message| sign_in::error(message, cx)))
        .into_any_element()
    }

    fn sign_in_now(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(form) = &mut self.sign_in else {
            return;
        };
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let (method, params) = if form.with_token {
            (
                "loginWithToken",
                json!({ "url": value(&form.url), "token": value(&form.token), "allow_insecure_private_http": form.insecure }),
            )
        } else {
            (
                "login",
                json!({
                    "url": value(&form.url),
                    "user": value(&form.user),
                    "password": value(&form.password),
                    "allow_insecure_private_http": form.insecure,
                }),
            )
        };
        form.busy = true;
        let call = host.call(method, params);
        after::<Rundeck, _>(
            call,
            |rd, host, done, cx| {
                match done {
                    Ok(_) => rd.sign_in = None,
                    Err(failure) => {
                        if let Some(form) = &mut rd.sign_in {
                            form.busy = false;
                        }
                        host.said(&failure.titled("Sign in to Rundeck"), cx);
                    }
                }
                host.read_again(None);
            },
            cx,
        );
        cx.notify();
    }
}
