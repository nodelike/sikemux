//! Signing in to a Jira site with an API token.

use super::*;

impl Jira {
    pub(super) fn sign_in(
        &mut self,
        host: &Host,
        status: &Status,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        if self.form.is_none() {
            let mut made = Vec::new();
            for (placeholder, masked) in [
                ("your-team.atlassian.net", false),
                ("you@example.com", false),
                ("ATATT…", true),
            ] {
                let mut slot = None;
                made.push(field(
                    &mut slot,
                    &mut self.typing,
                    placeholder,
                    masked,
                    (None, None),
                    window,
                    cx,
                ));
            }
            let [site, email, token] = <[_; 3]>::try_from(made).expect("three fields");
            self.form = Some(TokenForm {
                site,
                email,
                token,
                busy: false,
            });
        }
        let form = self.form.as_ref().expect("made just now");
        let filled = [&form.site, &form.email, &form.token]
            .iter()
            .all(|input| !input.read(cx).value().trim().is_empty());
        let browser = status.details.browser_sign_in;
        let message = status.message.clone();
        sign_in::Screen::new(
            "jira-sign-in",
            ui::icon(Icon::Jira, 28.).text_color(jira_blue()),
            "Sign in to Jira",
        )
        .lede("The tickets you are working on, beside the code, and tools for your agents to read and update them.")
        .foot(ui::sign_in::KEPT)
        .when(browser, |screen| {
            screen
                .child(crate::plugin_pane::browser_sign_in(
                    host,
                    "jira-browser",
                    Icon::Jira,
                    "Continue with Atlassian",
                    |host, cx| host.sign_in_with_browser(cx),
                    cx,
                ))
                .child(sign_in::or("or use an API token", cx))
        })
        .child(sign_in::Field::new("Jira site", &form.site))
        .child(sign_in::Field::new("Atlassian account email", &form.email))
        .child(sign_in::Field::new("API token", &form.token).mono())
        .child(
            Button::new(
                "jira-token-sign-in",
                if form.busy { "Signing in…" } else { "Sign in" },
                if browser {
                    Variant::Outline
                } else {
                    Variant::Primary
                },
            )
            .wide()
            .disabled(!filled || form.busy)
            .on_click(listen::<Jira, _>(cx, |jira, host, _, _, cx| {
                jira.sign_in_with_token(host, cx)
            })),
        )
        .children(message.map(|message| sign_in::error(message, cx)))
        .child(
            sign_in::links().child(
                sign_in::link("jira-make-token", "Create an API token", cx)
                    .on_click(|_, _, cx| cx.open_url(API_TOKENS_PAGE)),
            ),
        )
        .into_any_element()
    }

    fn sign_in_with_token(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(form) = &mut self.form else {
            return;
        };
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let params = json!({
            "site": value(&form.site),
            "email": value(&form.email),
            "token": value(&form.token),
        });
        form.busy = true;
        let call = host.call("signIn", params);
        after::<Jira, _>(
            call,
            |jira, host, done, cx| {
                if let Some(form) = &mut jira.form {
                    form.busy = false;
                }
                if let Err(failure) = done {
                    host.said(&failure.titled("Sign in to Jira"), cx);
                }
            },
            cx,
        );
        cx.notify();
    }
}
