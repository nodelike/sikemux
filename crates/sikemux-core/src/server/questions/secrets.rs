use super::*;

impl Questions {
    pub(super) fn keep_secret(
        &self,
        question: &Question,
        answer: &str,
    ) -> Result<std::path::PathBuf, String> {
        let agent = Path::new(&question.agent_id);
        let folder = self
            .secrets
            .as_ref()
            .filter(|_| agent.file_name() == Some(agent.as_os_str()))
            .ok_or("Sikemux has nowhere private to keep a secret")?
            .join(agent);
        let path = folder.join(&question.id);
        sikemux_process::files::write_atomic(&path, answer.as_bytes(), true)
            .map_err(|error| format!("Sikemux could not keep the secret: {error}"))?;
        let mut kept: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .collect();
        kept.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        for (_, old) in kept.iter().skip(MAX_SECRETS_PER_AGENT) {
            let _ = std::fs::remove_file(old);
        }
        if let Some(root) = self.secrets.as_deref() {
            *self.secrets_cache.locked() = Self::load_secrets(root);
        }
        Ok(path)
    }

    /// The text with every secret the person answered an agent with drawn as dots, so an agent
    /// passing one on never puts it in front of another agent or the person.
    pub(crate) fn hide_secrets(&self, text: &str) -> String {
        let kept = self.secrets_cache.locked();
        if kept.is_empty() {
            return text.to_owned();
        }
        kept.iter().fold(text.to_owned(), |text, secret| {
            text.replace(secret.as_str(), HIDDEN)
        })
    }

    pub(super) fn load_secrets(root: &Path) -> Vec<String> {
        let mut kept: Vec<String> = (std::fs::read_dir(root).into_iter().flatten().flatten())
            .flat_map(|agent| {
                std::fs::read_dir(agent.path())
                    .into_iter()
                    .flatten()
                    .flatten()
            })
            .filter_map(|file| std::fs::read_to_string(file.path()).ok())
            .map(|secret| secret.trim().to_owned())
            .filter(|secret| secret.chars().count() >= MIN_HIDDEN_SECRET_CHARS)
            .collect();
        kept.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        kept.dedup();
        kept
    }

    /// Copies the pictures of the raise's evidence, as [`Self::keep_pictures`] copies choices'.
    pub(crate) fn keeps_picture(&self, path: &Path) -> bool {
        (self.pictures.as_deref()).is_some_and(|dir| crate::server::attachments::is_kept(dir, path))
    }

    /// Copies the picture each choice names, a path from `folder`, into the core's own folder,
    /// pointing the choice at the copy. Nothing is copied unless every picture can be.
    pub(super) fn keep_pictures(
        &self,
        folder: &str,
        choices: &mut [QuestionChoice],
    ) -> Result<(), String> {
        let given: Vec<std::path::PathBuf> = (choices.iter())
            .filter_map(|choice| choice.picture.as_deref())
            .map(|picture| picture_file(folder, picture))
            .collect::<Result<_, _>>()?;
        if given.is_empty() {
            return Ok(());
        }
        let dir = (self.pictures.as_ref()).ok_or("Sikemux has nowhere to keep pictures")?;
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("Sikemux could not keep the pictures: {error}"))?;
        let shown = choices.iter_mut().filter(|choice| choice.picture.is_some());
        for (choice, from) in shown.zip(given) {
            let extension = (from.extension()).map_or(String::new(), |extension| {
                extension.to_string_lossy().to_lowercase()
            });
            let to = dir.join(format!("{}.{extension}", uuid::Uuid::new_v4().simple()));
            std::fs::copy(&from, &to)
                .map_err(|error| format!("Sikemux could not keep {}: {error}", from.display()))?;
            choice.picture = Some(to.to_string_lossy().into_owned());
        }
        Ok(())
    }
}

/// The image file a choice's `picture` names, from `folder` when it is not a whole path.
pub(super) fn picture_file(folder: &str, picture: &str) -> Result<std::path::PathBuf, String> {
    let path = Path::new(folder).join(picture);
    let extension = (path.extension()).map(|extension| extension.to_string_lossy().to_lowercase());
    if !extension.is_some_and(|extension| PICTURE_TYPES.contains(&extension.as_str())) {
        return Err(format!(
            "picture {picture} is not a PNG, JPEG, GIF or WebP file"
        ));
    }
    match std::fs::metadata(&path) {
        Ok(file) if file.is_file() && file.len() <= MAX_PICTURE_BYTES => Ok(path),
        Ok(file) if file.is_file() => Err(format!("picture {picture} is over 20 MB")),
        _ => Err(format!("picture {picture} is not a file Sikemux can read")),
    }
}

/// What the person typed for a question whose answer is never kept, put in a file only their
/// account can open: what the asker is told in its place.
pub(super) async fn kept_privately(
    core: &Arc<Core>,
    question: &Question,
    typed: String,
) -> Result<String, String> {
    let secret = tokio::task::spawn_blocking(move || sikemux_keychain::reference::resolve(&typed))
        .await
        .map_err(|error| error.to_string())
        .flatten()?;
    let path = core.harness().questions.keep_secret(question, &secret)?;
    Ok(secret_note(&path))
}

/// What the agent is told in place of the secret the person answered with.
pub(super) fn secret_note(path: &Path) -> String {
    let path = path.display();
    format!(
        "The person's answer is a secret, so Sikemux kept it out of this conversation. It is in {path}, which only the person's account can read. Use it straight from the file, such as \"$(cat '{path}')\" in a command, and never print it, echo it, or write it into a reply, a message or another file."
    )
}

/// Lists the pictures of the choices picked among the answer's files, copied to where the
/// chat's files are kept, so the answer shows what was picked once the question is gone.
pub(super) fn attach_picked_pictures(core: &Arc<Core>, answers: &mut [AnswerParams]) {
    let Some(dir) = core.config().attachment_dir.as_deref() else {
        return;
    };
    let shown: Vec<(usize, String)> = {
        let state = core.harness().questions.state.locked();
        (answers.iter().enumerate())
            .filter_map(|(at, given)| {
                let question = state.open.iter().find(|open| open.id == given.id)?;
                Some(
                    (given.choice.into_iter())
                        .filter_map(|ix| question.choices.get(ix)?.picture.clone())
                        .map(move |picture| (at, picture))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect()
    };
    for (at, picture) in shown {
        let from = Path::new(&picture);
        let Some(name) = from.file_name() else {
            continue;
        };
        let to = dir.join(name);
        let copied = std::fs::create_dir_all(dir).and_then(|()| std::fs::copy(from, &to));
        if let (Ok(_), Some(answer)) = (copied, answers.get_mut(at)) {
            answer.paths.push(to.to_string_lossy().into_owned());
        }
    }
}
