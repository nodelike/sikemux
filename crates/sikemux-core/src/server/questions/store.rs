use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "stage")]
pub(super) enum Stage {
    /// Sent to the agent that started the asker. `ended_at` is when that
    /// agent's turn ended after it was handed the question.
    #[serde(rename_all = "camelCase")]
    Lead {
        lead_id: String,
        lead_name: String,
        ended_at: Option<u64>,
    },
    /// Put to the person. `lead` is the agent it went to first, which left it unanswered.
    #[serde(rename_all = "camelCase")]
    Person { lead: Option<(String, String)> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Question {
    pub(super) id: String,
    pub(super) agent_id: String,
    pub(super) name: String,
    pub(super) cli: String,
    pub(super) project: String,
    /// The asker's own words, which its chat shows the question under.
    pub(super) asked: String,
    /// What the person is asked, which a lead that passes it on may reword.
    pub(super) text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) header: Option<String>,
    pub(super) choices: Vec<QuestionChoice>,
    /// The asker waits for the answer; otherwise it goes on and hears the answer later.
    pub(super) blocked: bool,
    /// The answer is a secret, such as a password, so the question goes
    /// straight to the person and the answer never enters a transcript.
    #[serde(default)]
    pub(super) secret: bool,
    /// What the asker says makes it the person's to decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) kind: Option<CallKind>,
    /// The agent whose work it is about, when that is not the asker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) about: Option<CallAsker>,
    /// The page that shows what it is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) page: Option<CallPage>,
    /// Pages after the first, when it names several.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) more_pages: Vec<CallPage>,
    /// Where a do call's task is done, or the change a look call shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) link: Option<CallLink>,
    /// What the asker, or the agent it is about, opened for the person just before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) opened: Vec<CallOpened>,
    pub(super) at: u64,
    pub(super) stage: Stage,
    /// The linked host it was also put to the person on, where its asker's team
    /// is led from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) relayed: Option<String>,
    /// The host it was relayed to took it, so that host lists it for phones and this one does not.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) relay_taken: bool,
    /// Why the answer the person gave it here never reached its asker on another host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) undelivered: Option<String>,
    /// The linked host its asker runs on, when it is put to the person here
    /// for an agent there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) from_host: Option<String>,
    /// The plugin tool call the person approves or turns down with their
    /// answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) approval: Option<Approval>,
    /// The dialog in the asker's process that waits on the answer, which then
    /// goes to it rather than arriving as a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) dialog: Option<String>,
    /// The questions the agent it is about asked its asker, which wait for this one's answer
    /// rather than going to the person as well.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) holds: Vec<String>,
}

impl Question {
    pub(super) fn view(&self) -> AgentQuestion {
        let (state, via_note) = match &self.stage {
            Stage::Lead { .. } => (QuestionState::Lead, None),
            Stage::Person { lead } => (
                QuestionState::Open,
                lead.as_ref()
                    .map(|(_, name)| format!("@{name} didn't answer")),
            ),
        };
        AgentQuestion {
            id: self.id.clone(),
            agent_id: self.agent_id.clone(),
            name: self.name.clone(),
            cli: self.cli.clone(),
            project: self.project.clone(),
            asked: self.asked.clone(),
            text: self.text.clone(),
            header: self.header.clone(),
            choices: self.choices.clone(),
            multi: false,
            blocked: self.blocked,
            assumption: None,
            secret: self.secret,
            at: self.at,
            state,
            via_note: self.undelivered.clone().or(via_note),
            secret_note: self.secret.then(|| format!(
                "Kept private. {} reads it from a file only your account can open, and it never shows in the chat.",
                self.name
            )),
            answer: None,
            ticket: None,
            production: self.production_call(),
        }
    }

    /// Every page it names, the first one first.
    pub(super) fn pages(&self) -> impl Iterator<Item = &CallPage> {
        self.page.iter().chain(&self.more_pages)
    }

    /// Its call to the person is one for a change to production.
    pub(super) fn production_call(&self) -> bool {
        !self.secret && self.approval.is_some()
    }

    /// The option of its call that lets a plugin call it holds run.
    pub(super) fn allows(&self) -> Option<String> {
        let approving = self.approval.as_ref()?.approving()?;
        Some((approving + 1).to_string())
    }

    /// It as a call to the person.
    pub(super) fn call(&self) -> PersonCall {
        let kind = match () {
            _ if self.secret => CallKind::Access,
            _ if self.approval.is_some() => CallKind::Production,
            _ => self.kind.unwrap_or(CallKind::Taste),
        };
        let (options, pick) = match kind {
            CallKind::Do => (form_options(&[DO_DONE, DO_NOT_DOING], &self.choices), None),
            CallKind::Look => (form_options(&[LOOK_SEEN], &self.choices), None),
            _ => call_options(&self.choices),
        };
        PersonCall {
            id: self.id.clone(),
            project: self.project.clone(),
            kind,
            title: self.header.clone(),
            asker: CallAsker {
                agent_id: self.agent_id.clone(),
                name: self.name.clone(),
                cli: self.cli.clone(),
                chief: false,
            },
            joined: Vec::new(),
            about: self.about.clone(),
            page: self.page.clone(),
            more_pages: self.more_pages.clone(),
            shown_in: Vec::new(),
            link: self.link.clone(),
            question: self.text.clone(),
            headline: String::new(),
            context: None,
            assumption: None,
            options,
            pick,
            against: None,
            evidence: Vec::new(),
            opened: self.opened.clone(),
            action: None,
            brief: None,
            place: None,
            can_hand_back: false,
            multi: false,
            secret: self.secret,
            raised_at: self.at,
            state: CallState::Open,
            answer: None,
            outcome: None,
        }
    }

    /// Whether this host lists it where every host's questions are gathered, as phones and the
    /// inbox do: a question another host took to put to the person is listed there alone.
    pub(super) fn listed_for_devices(&self) -> bool {
        self.asks_person() && !(self.relayed.is_some() && self.relay_taken)
    }

    pub(super) fn asks_person(&self) -> bool {
        matches!(self.stage, Stage::Person { .. })
    }

    pub(super) fn is_about(&self, agent: &str) -> bool {
        (self.about.as_ref()).is_some_and(|about| about.agent_id == agent)
    }

    /// Its id and title, as a list of calls names it to an agent.
    pub(super) fn listed(&self) -> String {
        let title = self.header.as_deref().unwrap_or(&self.text);
        format!("{} \"{title}\"", self.id)
    }

    pub(super) fn lead_id(&self) -> Option<&str> {
        match &self.stage {
            Stage::Lead { lead_id, .. } => Some(lead_id),
            Stage::Person { .. } => None,
        }
    }

    /// Whether `lead` was handed the question, before or after it went on
    /// to the person.
    pub(super) fn went_to(&self, lead: &str) -> bool {
        match &self.stage {
            Stage::Lead { lead_id, .. } => lead_id == lead,
            Stage::Person {
                lead: Some((lead_id, _)),
            } => lead_id == lead,
            Stage::Person { lead: None } => false,
        }
    }
}

/// The questions still open, kept in the data folder.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuestionsRecord {
    pub(super) open: Vec<Question>,
}

/// A question the person answered, kept until its asker reads the answer.
#[derive(Clone)]
pub(super) struct Answered {
    pub(super) question: Question,
    /// What it was answered with; none for a secret.
    pub(super) answer: Option<String>,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) open: Vec<Question>,
    /// The rule each agent was last sent back to over what it offered, so the same rule does not
    /// send it back again until a turn ends offering nothing a rule is about.
    pub(super) followed_rules: HashMap<String, String>,
    /// The agents whose last turn ended saying they wait with nothing set to wake them: false when
    /// that turn sent them back, true when they ended so again after being sent back.
    pub(super) unwoken: HashMap<String, bool>,
    /// Oldest first.
    pub(super) answered: Vec<Answered>,
    /// Something changed that the window has not been sent yet.
    pub(super) dirty: bool,
    /// The window connection last sent the questions.
    pub(super) told_window: Option<ClientId>,
    /// The person's open questions phones were told of, and whose they are.
    pub(super) told_phones: HashMap<String, String>,
    /// Every question put to the person that devices'
    /// views last showed, and whose they are.
    pub(super) shown_devices: HashMap<String, String>,
    pub(super) keeper: Keeper,
    /// The open questions also put to the person on another host, and which.
    pub(super) relayed_to: HashMap<String, String>,
    /// Notes for askers whose pick was cleared, by asker, not yet sent.
    pub(super) unpicked: Vec<(String, String)>,
}

pub(crate) struct Questions {
    pub(super) state: Mutex<State>,
    pub(super) kept: Kept,
    pub(super) secrets: Option<std::path::PathBuf>,
    pub(super) secrets_cache: Mutex<Vec<String>>,
    pub(super) pictures: Option<std::path::PathBuf>,
    /// Moves questions on and tells the window and phones of them.
    pub(crate) wake: Wake,
}

#[derive(Clone)]
pub(crate) struct ForPerson {
    pub id: String,
    pub agent_id: String,
    pub text: String,
    pub blocked: bool,
    /// It asks for a secret, such as a password or a token.
    pub secret: bool,
    /// The answers it can be given in one tap, in order.
    pub answers: Vec<String>,
    /// The picture each of `answers` shows, when its asker gave them.
    pub pictures: Vec<Option<String>>,
    pub at: u64,
}

/// A question's choices as a call's options, each named by its place, and the one marked as the pick.
pub(super) fn call_options(choices: &[QuestionChoice]) -> (Vec<CallOption>, Option<String>) {
    let id = |index: usize| (index + 1).to_string();
    let options = (choices.iter().enumerate())
        .map(|(index, choice)| CallOption {
            id: id(index),
            label: choice.label.clone(),
            detail: choice.detail.clone(),
            picture: choice.picture.clone(),
        })
        .collect();
    let pick = choices.iter().position(|choice| choice.recommended).map(id);
    (options, pick)
}

/// The fixed answers of a do or look call, by the ids clients know them by, over the choices the
/// question holds in the same order.
pub(super) fn form_options(ids: &[&str], choices: &[QuestionChoice]) -> Vec<CallOption> {
    (ids.iter().zip(choices))
        .map(|(id, choice)| CallOption {
            id: (*id).to_owned(),
            label: choice.label.clone(),
            detail: None,
            picture: None,
        })
        .collect()
}

/// The choices a do or look call is answered by, in the order of [`form_options`].
pub(super) fn form_choices(kind: CallKind) -> Option<Vec<QuestionChoice>> {
    let labels: &[&str] = match kind {
        CallKind::Do => &["Done", "Not doing"],
        CallKind::Look => &["Seen"],
        _ => return None,
    };
    Some(
        (labels.iter())
            .map(|label| QuestionChoice {
                label: (*label).to_owned(),
                detail: None,
                picture: None,
                recommended: false,
            })
            .collect(),
    )
}

pub(super) struct Asked {
    pub(super) text: String,
    pub(super) header: Option<String>,
    pub(super) choices: Vec<QuestionChoice>,
    pub(super) blocked: bool,
    pub(super) secret: bool,
    pub(super) kind: Option<CallKind>,
    pub(super) about: Option<CallAsker>,
    pub(super) pages: Vec<CallPage>,
    pub(super) link: Option<CallLink>,
    pub(super) approval: Option<Approval>,
    pub(super) dialog: Option<String>,
}

impl Questions {
    /// Takes up the questions left open by agents `known` says still exist.
    pub(crate) fn new(data_dir: Option<&Path>, known: impl Fn(&str) -> bool) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let mut record: QuestionsRecord = kept.load();
        let mut keeper = Keeper::new(data_dir.is_some());
        record.open.retain_mut(|question| {
            let known = known(&question.agent_id);
            match &mut question.approval {
                Some(approval) if known => keeper.restore(&question.id, approval),
                Some(_) => {
                    keeper.forget(&question.id);
                    false
                }
                None => known,
            }
        });
        let secrets = data_dir.map(|dir| dir.join(SECRETS));
        if let Some(entries) = secrets
            .as_deref()
            .and_then(|dir| std::fs::read_dir(dir).ok())
        {
            for entry in entries.flatten() {
                if !known(&entry.file_name().to_string_lossy()) {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        let pictures = data_dir.map(|dir| dir.join(PICTURES));
        if let Some(entries) = pictures
            .as_deref()
            .and_then(|dir| std::fs::read_dir(dir).ok())
        {
            let chosen = (record.open.iter())
                .flat_map(|question| question.choices.iter())
                .filter_map(|choice| choice.picture.clone());
            let shown: HashSet<String> = chosen.collect();
            for entry in entries.flatten() {
                if !shown.contains(&*entry.path().to_string_lossy()) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let secrets_cache = Mutex::new(
            secrets
                .as_deref()
                .map(Self::load_secrets)
                .unwrap_or_default(),
        );
        Self {
            secrets,
            secrets_cache,
            pictures,
            state: Mutex::new(State {
                dirty: !record.open.is_empty(),
                open: record.open,
                keeper,
                ..State::default()
            }),
            kept,
            wake: Wake::default(),
        }
    }

    pub(crate) fn has_open(&self, agent_id: &str) -> bool {
        self.state
            .locked()
            .open
            .iter()
            .any(|question| question.agent_id == agent_id)
    }

    /// The open questions changed: the window is told, and they are kept.
    pub(super) fn changed(&self, state: &mut State) {
        self.kept.save(&QuestionsRecord {
            open: state.open.clone(),
        });
        let waiting = state
            .open
            .iter()
            .filter_map(|question| Some((question.id.as_str(), question.approval.as_ref()?)));
        state.keeper.keep(waiting);
        state.dirty = true;
        self.wake.now();
    }

    pub(crate) fn is_open(&self, id: &str) -> bool {
        (self.state.locked().open.iter()).any(|question| question.id == id)
    }

    pub(crate) fn open_ids(&self) -> HashSet<String> {
        (self.state.locked().open.iter())
            .map(|question| question.id.clone())
            .collect()
    }

    pub(crate) fn blocks_on_person(&self, agent_id: &str) -> bool {
        self.state.locked().open.iter().any(|question| {
            question.agent_id == agent_id && question.blocked && question.asks_person()
        })
    }

    /// The question the agent stopped for that waits on the person, by id.
    pub(crate) fn blocking(&self, agent_id: &str) -> Option<String> {
        (self.state.locked().open.iter())
            .find(|question| {
                question.agent_id == agent_id && question.blocked && question.asks_person()
            })
            .map(|question| question.id.clone())
    }

    /// Whether the agent stopped to wait for the answer to a question.
    pub(crate) fn waits_on_answer(&self, agent_id: &str) -> bool {
        self.state
            .locked()
            .open
            .iter()
            .any(|question| question.agent_id == agent_id && question.blocked)
    }

    /// Each agent that stopped for the answer to a question its lead has, with that lead's name
    /// and when it asked; its oldest such question when it has several.
    pub(crate) fn waiting_on_leads(&self) -> HashMap<String, crate::server::turn_state::OnAgent> {
        let mut waiting = HashMap::new();
        for question in self.state.locked().open.iter().filter(|q| q.blocked) {
            if let Stage::Lead {
                lead_id, lead_name, ..
            } = &question.stage
            {
                waiting.entry(question.agent_id.clone()).or_insert_with(|| {
                    crate::server::turn_state::OnAgent {
                        id: lead_id.clone(),
                        name: lead_name.clone(),
                        since: question.at,
                    }
                });
            }
        }
        waiting
    }

    /// The questions put to the person, oldest first: of those an agent waits on, only its
    /// latest.
    pub(crate) fn for_person(&self) -> Vec<ForPerson> {
        let state = self.state.locked();
        let questions: Vec<ForPerson> = (state.open.iter())
            .filter(|question| question.listed_for_devices())
            .map(|question| ForPerson {
                id: question.id.clone(),
                agent_id: question.agent_id.clone(),
                text: question.text.clone(),
                blocked: question.blocked,
                secret: question.secret,
                answers: (question.choices.iter())
                    .map(|choice| choice.label.clone())
                    .collect(),
                pictures: match question
                    .choices
                    .iter()
                    .all(|choice| choice.picture.is_none())
                {
                    true => Vec::new(),
                    false => (question.choices.iter())
                        .map(|choice| choice.picture.clone())
                        .collect(),
                },
                at: question.at,
            })
            .collect();
        let mut latest: HashMap<&str, (u64, &str)> = HashMap::new();
        for question in questions.iter().filter(|question| question.blocked) {
            let kept = latest.entry(&question.agent_id).or_insert((0, ""));
            if kept.1.is_empty() || kept.0 < question.at {
                *kept = (question.at, &question.id);
            }
        }
        let shown: HashSet<String> = latest.values().map(|(_, id)| (*id).to_owned()).collect();
        (questions.iter())
            .filter(|question| !question.blocked || shown.contains(&question.id))
            .cloned()
            .collect()
    }

    /// The agent began another turn, as when the person wrote to it. A question it stopped for
    /// no longer waits, unless the call it raised is still `open`: a call waits until the person
    /// answers it. A question it did not stop for, and a plugin call waiting for the person's
    /// approval, still wait.
    pub(crate) fn turn_started(&self, agent_id: &str, open: &[String]) {
        let mut state = self.state.locked();
        let before = state.open.len();
        state.open.retain(|question| {
            let waited_on = question.blocked && question.asks_person();
            question.agent_id != agent_id
                || !waited_on
                || question.approval.is_some()
                || open.contains(&question.id)
        });
        if state.open.len() != before {
            self.changed(&mut state);
        }
    }

    /// The boards changed, which may change the ticket a question names.
    pub(crate) fn boards_changed(&self) {
        let mut state = self.state.locked();
        if !state.open.is_empty() {
            state.dirty = true;
            self.wake.now();
        }
    }

    /// The question `id` that `agent_id` asked and `lead` was handed, in the
    /// asker's own words, while it is open.
    pub(crate) fn asked_of(&self, lead: &str, agent_id: &str, id: &str) -> Option<String> {
        self.state
            .locked()
            .open
            .iter()
            .find(|question| {
                question.id == id && question.agent_id == agent_id && question.went_to(lead)
            })
            .map(|question| question.asked.clone())
    }

    /// Why `lead` can't answer `id` for the agent `name`: the call is the lead's own, or no
    /// such question is open.
    pub(crate) fn not_asked(&self, lead: &str, name: &str, id: &str) -> String {
        let name = crate::server::names::at_name(name);
        let state = self.state.locked();
        let own =
            (state.open.iter()).find(|question| question.id == id && question.agent_id == lead);
        match own {
            Some(call) => format!(
                "{id} is your own open call \"{}\", not a question {name} asked you. If it is settled, withdraw it with ask, edit: \"{id}\" and withdraw: true; then send this without answers",
                call.header.as_deref().unwrap_or(&call.text)
            ),
            None => format!("{name} has no open question {id} that it asked you; it was answered already, or leave out answers to send this as a plain message"),
        }
    }

    /// `agent`'s open calls that `keep` lets through, as `id "title"` items.
    pub(super) fn calls_of(&self, agent: &str, keep: impl Fn(&Question) -> bool) -> Vec<String> {
        let state = self.state.locked();
        (state.open.iter())
            .filter(|question| question.agent_id == agent && keep(question))
            .map(Question::listed)
            .collect()
    }

    /// The calls of `agent`'s that wait on the person, as `id "title"` items.
    pub(crate) fn asks_person(&self, agent: &str) -> Vec<String> {
        self.calls_of(agent, Question::asks_person)
    }

    /// What `agent` is told with the person's message while calls of its own, or of others about
    /// its work, wait on them.
    pub(crate) fn open_calls_note(&self, agent: &str) -> Option<String> {
        let own = self.asks_person(agent);
        let own = (!own.is_empty()).then(|| {
            format!(
                "Your calls still open to the person: {}. If this message settles one, withdraw it with ask, edit: its id and withdraw: true.",
                own.join("; ")
            )
        });
        let others: Vec<String> = (self.state.locked().open.iter())
            .filter(|call| call.asks_person() && call.agent_id != agent && call.is_about(agent))
            .map(|call| {
                format!(
                    "{} from {}",
                    call.listed(),
                    crate::server::names::at_name(&call.name)
                )
            })
            .collect();
        let others = (!others.is_empty()).then(|| {
            format!(
                "Calls about your work still open to the person: {}. If this message settles one, tell the agent that raised it.",
                others.join("; ")
            )
        });
        let notes: Vec<String> = own.into_iter().chain(others).collect();
        (!notes.is_empty()).then(|| notes.join("\n"))
    }

    #[cfg(test)]
    pub(crate) fn hold(&self, question: serde_json::Value) {
        let question = serde_json::from_value(question).expect("a question");
        self.state.locked().open.push(question);
    }

    /// What `lead` is told with a report from `worker` while calls of the lead's about it are open.
    pub(crate) fn calls_about_note(&self, lead: &str, worker: &str) -> Option<String> {
        let calls = self.calls_of(lead, |call| call.is_about(worker));
        (!calls.is_empty()).then(|| {
            format!(
                "Your open calls about this agent: {}. If this report settles one, withdraw it with ask, edit: its id and withdraw: true.",
                calls.join("; ")
            )
        })
    }

    /// Closes the question `id` its agent asked `lead`, which answered it, while it is open.
    pub(super) fn take_answered(&self, lead: &str, agent_id: &str, id: &str) -> Option<Question> {
        let mut state = self.state.locked();
        let at = (state.open.iter()).position(|question| {
            question.id == id && question.agent_id == agent_id && question.went_to(lead)
        })?;
        let question = state.open.remove(at);
        self.changed(&mut state);
        Some(question)
    }

    /// The agent was handed the person's answers to its questions.
    pub(crate) fn delivered(&self, agent_id: &str) {
        let mut state = self.state.locked();
        let before = state.answered.len();
        state
            .answered
            .retain(|answered| answered.question.agent_id != agent_id);
        if state.answered.len() != before {
            self.changed(&mut state);
        }
    }

    /// The person wrote to `agent_id` directly, so a call about its work no longer stands behind
    /// the pick its asker made before: the pick is cleared and the asker told to edit or withdraw.
    pub(crate) fn steered(&self, agent_id: &str) {
        let mut state = self.state.locked();
        let mut notes = Vec::new();
        for question in &mut state.open {
            let about = question
                .about
                .as_ref()
                .filter(|about| about.agent_id == agent_id);
            let Some(about) = about.filter(|_| question.asks_person()) else {
                continue;
            };
            let picked = question
                .choices
                .iter_mut()
                .filter(|choice| choice.recommended);
            if picked.map(|choice| choice.recommended = false).count() == 0 {
                continue;
            }
            let named = question
                .header
                .clone()
                .unwrap_or_else(|| clipped(&question.text, 80));
            let note = format!(
                "The person wrote to @{} directly after your call \"{named}\", so its pick was \
                 cleared. If it still stands, give the pick again with ask edit: \"{}\" and pick; \
                 if not, edit it or withdraw it with withdraw: true.",
                about.name, question.id
            );
            notes.push((question.agent_id.clone(), note));
        }
        if !notes.is_empty() {
            state.unpicked.extend(notes);
            self.changed(&mut state);
        }
    }

    /// The person typed into the terminal of an agent that asked them
    /// something, which answers it there.
    pub(crate) fn typed(&self, agent_id: &str) {
        let mut state = self.state.locked();
        let before = state.open.len();
        state
            .open
            .retain(|question| !(question.agent_id == agent_id && question.asks_person()));
        if state.open.len() != before {
            self.changed(&mut state);
        }
    }
}
