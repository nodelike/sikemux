//! Inline LaTeX math written out as Unicode runs, such as `\alpha^2 \leq \frac{1}{n}` as
//! `α² ≤ 1/n`, so it sits in a line of text and wraps with it: letters in italic, numbers,
//! signs and names such as log upright, and a script Unicode has no letters for set smaller.

/// How a run of a formula is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub italic: bool,
    /// A sub- or superscript Unicode cannot write, set smaller.
    pub small: bool,
}

/// `tex` as runs of Unicode text; commands it does not know are kept as written.
pub fn runs(tex: &str) -> Vec<(String, Look)> {
    let mut reader = Reader {
        chars: tex.chars().collect(),
        at: 0,
    };
    let mut out = Out::default();
    reader.run(&mut out, None);
    out.tidy()
}

/// Runs being written, each kept whole while its look stays the same.
#[derive(Default)]
struct Out(Vec<(String, Look)>);

impl Out {
    fn push_str(&mut self, text: &str, look: Look) {
        if text.is_empty() {
            return;
        }
        match self.0.last_mut() {
            Some((last, kept)) if *kept == look => last.push_str(text),
            _ => self.0.push((text.to_owned(), look)),
        }
    }

    /// A character as math sets it: a Latin letter in italic, anything else upright.
    fn push(&mut self, char: char) {
        let look = Look {
            italic: char.is_ascii_alphabetic(),
            small: false,
        };
        self.push_str(char.encode_utf8(&mut [0; 4]), look);
    }

    fn upright(&mut self, text: &str) {
        self.push_str(text, Look::default());
    }

    fn append(&mut self, other: Out) {
        for (text, look) in other.0 {
            self.push_str(&text, look);
        }
    }

    fn text(&self) -> String {
        self.0.iter().map(|(text, _)| text.as_str()).collect()
    }

    /// The runs with runs of spaces made one and none at either end.
    fn tidy(self) -> Vec<(String, Look)> {
        let mut out = Out::default();
        let mut space = false;
        for (text, look) in self.0 {
            for char in text.chars() {
                if char.is_whitespace() {
                    space = true;
                    continue;
                }
                if space && !out.0.is_empty() {
                    out.push_str(" ", Look::default());
                }
                space = false;
                out.push_str(char.encode_utf8(&mut [0; 4]), look);
            }
        }
        out.0
    }
}

struct Reader {
    chars: Vec<char>,
    at: usize,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn next(&mut self) -> Option<char> {
        let char = self.peek()?;
        self.at += 1;
        Some(char)
    }

    /// Writes what follows until `until` or the end.
    fn run(&mut self, out: &mut Out, until: Option<char>) {
        while let Some(char) = self.next() {
            match char {
                _ if Some(char) == until => return,
                '{' => self.run(out, Some('}')),
                '\\' => self.command(out),
                '^' => self.script(out, SUPERSCRIPT),
                '_' => self.script(out, SUBSCRIPT),
                '~' | '&' => out.upright(" "),
                '\'' => out.upright("′"),
                '-' => out.upright("−"),
                _ => out.push(char),
            }
        }
    }

    /// The next argument: a braced group or one character or command, written out.
    fn argument(&mut self) -> Out {
        while self.peek() == Some(' ') {
            self.at += 1;
        }
        let mut out = Out::default();
        match self.next() {
            Some('{') => self.run(&mut out, Some('}')),
            Some('\\') => self.command(&mut out),
            Some(char) => out.push(char),
            None => {}
        }
        out
    }

    /// The argument read as it was written, for `\text` and the like.
    fn raw_argument(&mut self) -> String {
        while self.peek() == Some(' ') {
            self.at += 1;
        }
        if self.peek() != Some('{') {
            return self.argument().text();
        }
        self.at += 1;
        let mut depth = 1;
        let mut out = String::new();
        while let Some(char) = self.next() {
            match char {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            out.push(char);
        }
        out
    }

    /// A sub- or superscript: in Unicode's own small letters when it has them all, else
    /// the script set smaller.
    fn script(&mut self, out: &mut Out, table: &[(char, char)]) {
        let written = self.argument();
        let text = written.text();
        let raised: Option<String> = text
            .chars()
            .map(|char| {
                table
                    .iter()
                    .find(|(from, _)| *from == char)
                    .map(|(_, to)| *to)
            })
            .collect();
        match raised {
            Some(raised) if !text.is_empty() => out.upright(&raised),
            _ => {
                for (text, look) in written.0 {
                    out.push_str(
                        &text,
                        Look {
                            small: true,
                            ..look
                        },
                    );
                }
            }
        }
    }

    fn command(&mut self, out: &mut Out) {
        let mut name = String::new();
        while let Some(char) = self.peek().filter(char::is_ascii_alphabetic) {
            name.push(char);
            self.at += 1;
        }
        if name.is_empty() {
            match self.next() {
                Some(',' | ':' | ';' | ' ' | '\\') => out.upright(" "),
                Some('!') | None => {}
                Some(char) => out.upright(char.encode_utf8(&mut [0; 4])),
            }
            return;
        }
        match name.as_str() {
            "frac" | "dfrac" | "tfrac" => {
                let (top, bottom) = (self.argument(), self.argument());
                grouped(out, top);
                out.upright("/");
                grouped(out, bottom);
            }
            "sqrt" => {
                let inside = self.argument();
                out.upright("√");
                grouped(out, inside);
            }
            "text" | "textrm" | "textit" | "textbf" | "mathrm" | "mathbf" | "operatorname"
            | "mbox" | "mathsf" | "mathtt" | "boldsymbol" => {
                let words = self.raw_argument();
                out.upright(&words);
            }
            "mathit" => out.append(self.argument()),
            "mathbb" => {
                let inside: String = self.argument().text().chars().map(double_struck).collect();
                out.upright(&inside);
            }
            "mathcal" => out.append(self.argument()),
            "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "displaystyle" | "limits"
            | "nolimits" => {}
            "hat" | "bar" | "vec" | "tilde" | "dot" | "overline" => {
                let inside = self.argument();
                let single = inside.text().chars().count() == 1;
                out.append(inside);
                if single {
                    let mark = match name.as_str() {
                        "hat" => "\u{302}",
                        "bar" | "overline" => "\u{304}",
                        "vec" => "\u{20d7}",
                        "tilde" => "\u{303}",
                        _ => "\u{307}",
                    };
                    let look = out.0.last().map_or(Look::default(), |(_, look)| *look);
                    out.push_str(mark, look);
                }
            }
            "quad" | "qquad" => out.upright("  "),
            "log" | "ln" | "lg" | "exp" | "sin" | "cos" | "tan" | "sec" | "csc" | "cot"
            | "sinh" | "cosh" | "tanh" | "arcsin" | "arccos" | "arctan" | "max" | "min" | "lim"
            | "det" | "gcd" | "deg" | "dim" | "ker" | "arg" | "sup" | "inf" | "Pr" | "argmax"
            | "argmin" | "mod" | "bmod" => {
                out.upright(&name);
                out.upright(" ");
            }
            _ => match SYMBOLS.iter().find(|(command, _)| *command == name) {
                Some((_, symbol)) => {
                    let greek = symbol.chars().all(|char| ('α'..='ω').contains(&char));
                    out.push_str(
                        symbol,
                        Look {
                            italic: greek,
                            small: false,
                        },
                    )
                }
                None => {
                    out.upright("\\");
                    out.upright(&name);
                }
            },
        }
    }
}

/// A fraction's part, in brackets when it is more than one term.
fn grouped(out: &mut Out, part: Out) {
    let one = part
        .text()
        .chars()
        .all(|char| char.is_alphanumeric() || char == '.');
    if !one {
        out.upright("(");
    }
    out.append(part);
    if !one {
        out.upright(")");
    }
}

fn double_struck(char: char) -> char {
    match char {
        'C' => 'ℂ',
        'H' => 'ℍ',
        'N' => 'ℕ',
        'P' => 'ℙ',
        'Q' => 'ℚ',
        'R' => 'ℝ',
        'Z' => 'ℤ',
        'A'..='Z' => char::from_u32(0x1d538 + (char as u32 - 'A' as u32)).unwrap_or(char),
        _ => char,
    }
}

const SUPERSCRIPT: &[(char, char)] = &[
    ('0', '⁰'),
    ('1', '¹'),
    ('2', '²'),
    ('3', '³'),
    ('4', '⁴'),
    ('5', '⁵'),
    ('6', '⁶'),
    ('7', '⁷'),
    ('8', '⁸'),
    ('9', '⁹'),
    ('+', '⁺'),
    ('−', '⁻'),
    ('=', '⁼'),
    ('(', '⁽'),
    (')', '⁾'),
    ('a', 'ᵃ'),
    ('b', 'ᵇ'),
    ('c', 'ᶜ'),
    ('d', 'ᵈ'),
    ('e', 'ᵉ'),
    ('f', 'ᶠ'),
    ('g', 'ᵍ'),
    ('h', 'ʰ'),
    ('i', 'ⁱ'),
    ('j', 'ʲ'),
    ('k', 'ᵏ'),
    ('l', 'ˡ'),
    ('m', 'ᵐ'),
    ('n', 'ⁿ'),
    ('o', 'ᵒ'),
    ('p', 'ᵖ'),
    ('r', 'ʳ'),
    ('s', 'ˢ'),
    ('t', 'ᵗ'),
    ('u', 'ᵘ'),
    ('v', 'ᵛ'),
    ('w', 'ʷ'),
    ('x', 'ˣ'),
    ('y', 'ʸ'),
    ('z', 'ᶻ'),
    ('T', 'ᵀ'),
    ('′', '′'),
    (' ', ' '),
];

const SUBSCRIPT: &[(char, char)] = &[
    ('0', '₀'),
    ('1', '₁'),
    ('2', '₂'),
    ('3', '₃'),
    ('4', '₄'),
    ('5', '₅'),
    ('6', '₆'),
    ('7', '₇'),
    ('8', '₈'),
    ('9', '₉'),
    ('+', '₊'),
    ('=', '₌'),
    ('(', '₍'),
    (')', '₎'),
    ('a', 'ₐ'),
    ('e', 'ₑ'),
    ('i', 'ᵢ'),
    ('j', 'ⱼ'),
    ('k', 'ₖ'),
    ('n', 'ₙ'),
    ('h', 'ₕ'),
    ('l', 'ₗ'),
    ('m', 'ₘ'),
    ('o', 'ₒ'),
    ('p', 'ₚ'),
    ('r', 'ᵣ'),
    ('s', 'ₛ'),
    ('t', 'ₜ'),
    ('u', 'ᵤ'),
    ('v', 'ᵥ'),
    ('x', 'ₓ'),
    ('−', '₋'),
];

const SYMBOLS: &[(&str, &str)] = &[
    ("alpha", "α"),
    ("beta", "β"),
    ("gamma", "γ"),
    ("delta", "δ"),
    ("epsilon", "ε"),
    ("varepsilon", "ε"),
    ("zeta", "ζ"),
    ("eta", "η"),
    ("theta", "θ"),
    ("vartheta", "ϑ"),
    ("iota", "ι"),
    ("kappa", "κ"),
    ("lambda", "λ"),
    ("mu", "μ"),
    ("nu", "ν"),
    ("xi", "ξ"),
    ("pi", "π"),
    ("rho", "ρ"),
    ("sigma", "σ"),
    ("tau", "τ"),
    ("upsilon", "υ"),
    ("phi", "φ"),
    ("varphi", "φ"),
    ("chi", "χ"),
    ("psi", "ψ"),
    ("omega", "ω"),
    ("Gamma", "Γ"),
    ("Delta", "Δ"),
    ("Theta", "Θ"),
    ("Lambda", "Λ"),
    ("Xi", "Ξ"),
    ("Pi", "Π"),
    ("Sigma", "Σ"),
    ("Phi", "Φ"),
    ("Psi", "Ψ"),
    ("Omega", "Ω"),
    ("cdot", "·"),
    ("times", "×"),
    ("div", "÷"),
    ("pm", "±"),
    ("mp", "∓"),
    ("leq", "≤"),
    ("le", "≤"),
    ("geq", "≥"),
    ("ge", "≥"),
    ("neq", "≠"),
    ("ne", "≠"),
    ("approx", "≈"),
    ("sim", "∼"),
    ("simeq", "≃"),
    ("equiv", "≡"),
    ("propto", "∝"),
    ("ll", "≪"),
    ("gg", "≫"),
    ("infty", "∞"),
    ("sum", "∑"),
    ("prod", "∏"),
    ("int", "∫"),
    ("oint", "∮"),
    ("partial", "∂"),
    ("nabla", "∇"),
    ("in", "∈"),
    ("notin", "∉"),
    ("ni", "∋"),
    ("subset", "⊂"),
    ("subseteq", "⊆"),
    ("supset", "⊃"),
    ("supseteq", "⊇"),
    ("cup", "∪"),
    ("cap", "∩"),
    ("emptyset", "∅"),
    ("varnothing", "∅"),
    ("forall", "∀"),
    ("exists", "∃"),
    ("neg", "¬"),
    ("lnot", "¬"),
    ("land", "∧"),
    ("wedge", "∧"),
    ("lor", "∨"),
    ("vee", "∨"),
    ("oplus", "⊕"),
    ("otimes", "⊗"),
    ("circ", "∘"),
    ("to", "→"),
    ("rightarrow", "→"),
    ("leftarrow", "←"),
    ("gets", "←"),
    ("leftrightarrow", "↔"),
    ("Rightarrow", "⇒"),
    ("implies", "⇒"),
    ("Leftarrow", "⇐"),
    ("Leftrightarrow", "⇔"),
    ("iff", "⇔"),
    ("mapsto", "↦"),
    ("uparrow", "↑"),
    ("downarrow", "↓"),
    ("ldots", "…"),
    ("dots", "…"),
    ("cdots", "⋯"),
    ("vdots", "⋮"),
    ("ddots", "⋱"),
    ("prime", "′"),
    ("degree", "°"),
    ("angle", "∠"),
    ("perp", "⊥"),
    ("parallel", "∥"),
    ("mid", "∣"),
    ("langle", "⟨"),
    ("rangle", "⟩"),
    ("lceil", "⌈"),
    ("rceil", "⌉"),
    ("lfloor", "⌊"),
    ("rfloor", "⌋"),
    ("lvert", "|"),
    ("rvert", "|"),
    ("vert", "|"),
    ("Vert", "‖"),
    ("lVert", "‖"),
    ("rVert", "‖"),
    ("ell", "ℓ"),
    ("hbar", "ℏ"),
    ("Re", "ℜ"),
    ("Im", "ℑ"),
    ("aleph", "ℵ"),
    ("star", "⋆"),
    ("ast", "∗"),
    ("bullet", "•"),
    ("checkmark", "✓"),
    ("lbrace", "{"),
    ("rbrace", "}"),
    ("{", "{"),
    ("}", "}"),
];
