//! What a project's checked-in commands ask of the person before they run, one question at a
//! time.

use crate::core::Target;
use crate::hosts::Place;
use sikemux_calls::call;
use sikemux_calls::tasks::TrustAnswerParams;
use std::collections::HashSet;

use gpui_kit::Context;
use sikemux_calls::tasks::TrustQuestion;

use crate::workbench::Workbench;

/// The trust questions already put to the person.
#[derive(Default)]
pub(crate) struct Trust {
    /// By the host that asks and the question's id.
    asked: HashSet<String>,
}

impl Trust {
    /// This host's questions, then the account host's, each under the host that asks: the first
    /// not put to the person yet, now marked as put, while `free`. Forgets those no longer asked.
    pub(crate) fn next(
        &mut self,
        feeds: &crate::hosts::Feeds,
        free: bool,
    ) -> Option<(Option<String>, TrustQuestion)> {
        let front = feeds
            .front()
            .and_then(|host| Some((host, feeds.host(host)?)));
        let asks: Vec<(Option<String>, &TrustQuestion)> =
            (feeds.this.trust.iter())
                .map(|ask| (None, ask))
                .chain(front.into_iter().flat_map(|(host, feed)| {
                    feed.trust.iter().map(|ask| (Some(host.clone()), ask))
                }))
                .collect();
        let asked_as = |host: &Option<String>, ask: &TrustQuestion| {
            format!("{}:{}", host.as_deref().unwrap_or_default(), ask.id)
        };
        self.asked
            .retain(|asked| asks.iter().any(|(host, ask)| &asked_as(host, ask) == asked));
        if !free {
            return None;
        }
        let (host, ask) = asks
            .into_iter()
            .find(|(host, ask)| !self.asked.contains(&asked_as(host, ask)))?;
        self.asked.insert(asked_as(&host, ask));
        Some((host, ask.clone()))
    }
}

impl Workbench {
    /// Puts the core's next trust question to the person, each once, while nothing else is asked.
    pub(crate) fn ask_trust(&mut self, cx: &mut Context<Self>) {
        let free = self.overlays.asking.is_none();
        let Some((host, ask)) = self.trust.next(&self.feeds, free) else {
            return;
        };
        let id = ask.id.clone();
        let on = host.as_ref().and_then(|host| {
            self.host_choices()
                .into_iter()
                .find(|option| option.choice == crate::hosts::HostChoice::Account(host.clone()))
                .map(|option| option.label)
        });
        let commands: Vec<String> = (ask.commands.iter())
            .map(|command| format!("{}: {}", command.label, command.command))
            .collect();
        let body: Vec<String> = [ask.body.clone(), commands.join("\n")]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        #[cfg(feature = "drive")]
        crate::drive::trace(format_args!("trust question set"));
        let title = match on {
            Some(name) => t!("{title} on {host}", title = ask.title, host = name),
            None => ask.title.clone(),
        };
        self.confirm(
            title,
            body.join("\n\n"),
            ask.confirm_label,
            false,
            move |bench, trust, _, _| {
                let place = host.clone().map_or(Place::This, Place::Account);
                Target::new(&bench.asks, place).call_typed::<call::AnswerTrust>(
                    TrustAnswerParams {
                        id: id.clone(),
                        trust,
                    },
                );
            },
            cx,
        );
    }
}
