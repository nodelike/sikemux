//! The tokens a transcript reports, as each agent's reader of its own transcript tallies them.

#[derive(Default, Debug, PartialEq, Clone, Copy)]
pub struct TokenCount {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenCount {
    pub fn is_empty(&self) -> bool {
        self.input + self.output + self.cache_read + self.cache_write == 0
    }

    pub fn add(&mut self, other: TokenCount) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

/// Tokens billed at one model and speed. Speed is `standard`, `fast` or `ultrafast`.
#[derive(Default, Debug, PartialEq)]
pub struct ModelTokens {
    pub model: String,
    pub speed: String,
    pub count: TokenCount,
}

#[derive(Default)]
pub struct ModelTally(pub Vec<ModelTokens>);

impl ModelTally {
    pub fn add(&mut self, model: &str, speed: &str, count: TokenCount) {
        match self
            .0
            .iter_mut()
            .find(|entry| entry.model == model && entry.speed == speed)
        {
            Some(entry) => entry.count.add(count),
            None => self.0.push(ModelTokens {
                model: model.to_string(),
                speed: speed.to_string(),
                count,
            }),
        }
    }
}

/// Where reading a transcript stopped. Codex names its model and speed once per
/// turn, ahead of the token reports, so both carry over to the next read.
#[derive(Default, Debug, PartialEq, Clone)]
pub struct ReadState {
    pub mark: String,
    pub model: String,
    pub speed: String,
}
