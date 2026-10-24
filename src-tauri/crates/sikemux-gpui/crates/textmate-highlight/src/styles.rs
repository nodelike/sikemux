use std::collections::HashMap;

/// How a token is drawn: one of the theme's roles, or the text colour when `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub role: Option<u16>,
    pub italic: bool,
    pub bold: bool,
    pub underline: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Font {
    italic: bool,
    bold: bool,
    underline: bool,
}

impl Font {
    fn parse(style: &str) -> Self {
        let has = |word| style.split_whitespace().any(|part| part == word);
        Self {
            italic: has("italic"),
            bold: has("bold"),
            underline: has("underline"),
        }
    }
}

/// A theme's token rules. A scope takes the colour of the longest selector it starts
/// with, and its font style from the longest that sets one; down a scope stack the
/// deepest scope with either wins, as in vscode-textmate.
pub struct Styles {
    roles: Vec<String>,
    rules: HashMap<String, (u16, Option<Font>)>,
}

impl Styles {
    pub fn new<'a>(rules: impl IntoIterator<Item = (&'a str, &'a str, Option<&'a str>)>) -> Self {
        let mut styles = Self {
            roles: Vec::new(),
            rules: HashMap::new(),
        };
        for (selector, role, font) in rules {
            let role = match styles.roles.iter().position(|known| known == role) {
                Some(index) => index,
                None => {
                    styles.roles.push(role.to_owned());
                    styles.roles.len() - 1
                }
            };
            styles
                .rules
                .insert(selector.to_owned(), (role as u16, font.map(Font::parse)));
        }
        styles
    }

    /// The role names, indexed by [`Style::role`].
    pub fn roles(&self) -> &[String] {
        &self.roles
    }

    pub fn style<'a>(&self, scopes: impl DoubleEndedIterator<Item = &'a str>) -> Style {
        let (mut role, mut font) = (None, None);
        for scope in scopes.rev() {
            let (scope_role, scope_font) = self.matching(scope);
            role = role.or(scope_role);
            font = font.or(scope_font);
            if role.is_some() && font.is_some() {
                break;
            }
        }
        let font = font.unwrap_or_default();
        Style {
            role,
            italic: font.italic,
            bold: font.bold,
            underline: font.underline,
        }
    }

    fn matching(&self, scope: &str) -> (Option<u16>, Option<Font>) {
        let mut role = None;
        let mut font = None;
        let mut prefix = scope;
        loop {
            if let Some((rule_role, rule_font)) = self.rules.get(prefix) {
                role = role.or(Some(*rule_role));
                font = font.or(*rule_font);
            }
            if role.is_some() && font.is_some() {
                break;
            }
            match prefix.rfind('.') {
                Some(dot) => prefix = &prefix[..dot],
                None => break,
            }
        }
        (role, font)
    }
}
