/// `claude-opus-5-5` reads as Opus 5.5 and `gpt-5.6-sol` as GPT-5.6 Sol; anything else keeps its id.
pub fn model_name(model: &str) -> String {
    let bare = model.rsplit('/').next().unwrap_or(model);
    let bare = bare.split('[').next().unwrap_or(bare);
    let capital = |word: &str| {
        let mut letters = word.chars();
        letters
            .next()
            .map(|first| first.to_uppercase().chain(letters).collect())
            .unwrap_or_default()
    };
    if let Some(rest) = bare.strip_prefix("claude-") {
        let mut parts: Vec<&str> = rest.split('-').collect();
        if parts
            .last()
            .is_some_and(|last| last.len() == 8 && last.chars().all(|c| c.is_ascii_digit()))
        {
            parts.pop();
        }
        let digits = |part: &&str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
        match parts.as_slice() {
            [family, major] if family.chars().all(|c| c.is_ascii_lowercase()) && digits(major) => {
                return format!("{} {major}", capital(family));
            }
            [family, major, minor]
                if family.chars().all(|c| c.is_ascii_lowercase())
                    && digits(major)
                    && digits(minor)
                    && minor.len() <= 2 =>
            {
                return format!("{} {major}.{minor}", capital(family));
            }
            _ => {}
        }
    }
    if let Some(rest) = bare.strip_prefix("gpt-") {
        let (version, variant) = rest.split_once('-').unwrap_or((rest, ""));
        if !version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.') {
            let variant: Vec<String> = variant
                .split('-')
                .filter(|word| !word.is_empty())
                .map(capital)
                .collect();
            return match variant.is_empty() {
                true => format!("GPT-{version}"),
                false => format!("GPT-{version} {}", variant.join(" ")),
            };
        }
    }
    bare.to_owned()
}
