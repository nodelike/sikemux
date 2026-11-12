//! Inline LaTeX math written out as plain Unicode, such as `\alpha^2 \leq \frac{1}{n}` as
//! `α² ≤ 1/n`, so it sits in a line of text and wraps with it.

/// `tex` as Unicode text; commands it does not know are kept as written.
pub fn unicode(tex: &str) -> String {
    let mut reader = Reader {
        chars: tex.chars().collect(),
        at: 0,
    };
    let mut out = String::new();
    reader.run(&mut out, None);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
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
    fn run(&mut self, out: &mut String, until: Option<char>) {
        while let Some(char) = self.next() {
            match char {
                _ if Some(char) == until => return,
                '{' => self.run(out, Some('}')),
                '\\' => self.command(out),
                '^' => self.script(out, SUPERSCRIPT, '^'),
                '_' => self.script(out, SUBSCRIPT, '_'),
                '~' => out.push(' '),
                '&' => out.push(' '),
                '\'' => out.push('′'),
                _ => out.push(char),
            }
        }
    }

    /// The next argument: a braced group or one character or command, written out.
    fn argument(&mut self) -> String {
        while self.peek() == Some(' ') {
            self.at += 1;
        }
        let mut out = String::new();
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
            return self.argument();
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

    fn script(&mut self, out: &mut String, table: &[(char, char)], mark: char) {
        let written = self.argument();
        let raised: Option<String> = written
            .chars()
            .map(|char| {
                table
                    .iter()
                    .find(|(from, _)| *from == char)
                    .map(|(_, to)| *to)
            })
            .collect();
        match raised {
            Some(raised) if !written.is_empty() => out.push_str(&raised),
            _ if written.chars().count() == 1 => {
                out.push(mark);
                out.push_str(&written);
            }
            _ => {
                out.push(mark);
                out.push('(');
                out.push_str(&written);
                out.push(')');
            }
        }
    }

    fn command(&mut self, out: &mut String) {
        let mut name = String::new();
        while let Some(char) = self.peek().filter(char::is_ascii_alphabetic) {
            name.push(char);
            self.at += 1;
        }
        if name.is_empty() {
            match self.next() {
                Some(',' | ':' | ';' | ' ') => out.push(' '),
                Some('!') => {}
                Some('\\') => out.push(' '),
                Some(char) => out.push(char),
                None => {}
            }
            return;
        }
        match name.as_str() {
            "frac" | "dfrac" | "tfrac" => {
                let (top, bottom) = (self.argument(), self.argument());
                out.push_str(&grouped(&top));
                out.push('/');
                out.push_str(&grouped(&bottom));
            }
            "sqrt" => {
                let inside = self.argument();
                out.push('√');
                out.push_str(&grouped(&inside));
            }
            "text" | "textrm" | "textit" | "textbf" | "mathrm" | "mathit" | "mathbf"
            | "operatorname" | "mbox" | "mathsf" | "mathtt" | "boldsymbol" => {
                out.push_str(&self.raw_argument())
            }
            "mathbb" => {
                let inside = self.argument();
                out.extend(inside.chars().map(double_struck));
            }
            "mathcal" => out.push_str(&self.argument()),
            "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "displaystyle" | "limits"
            | "nolimits" => {}
            "hat" | "bar" | "vec" | "tilde" | "dot" | "overline" => {
                let inside = self.argument();
                let mark = match name.as_str() {
                    "hat" => '\u{302}',
                    "bar" | "overline" => '\u{304}',
                    "vec" => '\u{20d7}',
                    "tilde" => '\u{303}',
                    _ => '\u{307}',
                };
                out.push_str(&inside);
                if inside.chars().count() == 1 {
                    out.push(mark);
                }
            }
            "quad" | "qquad" => out.push_str("  "),
            "log" | "ln" | "exp" | "sin" | "cos" | "tan" | "max" | "min" | "lim" | "det"
            | "gcd" | "deg" | "arg" | "sup" | "inf" | "Pr" => out.push_str(&name),
            _ => match SYMBOLS.iter().find(|(command, _)| *command == name) {
                Some((_, symbol)) => out.push_str(symbol),
                None => {
                    out.push('\\');
                    out.push_str(&name);
                }
            },
        }
    }
}

/// A fraction's part, in brackets when it is more than one term.
fn grouped(part: &str) -> String {
    match part
        .chars()
        .all(|char| char.is_alphanumeric() || char == '.')
    {
        true => part.to_owned(),
        false => format!("({part})"),
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
    ('-', '⁻'),
    ('−', '⁻'),
    ('=', '⁼'),
    ('(', '⁽'),
    (')', '⁾'),
    ('n', 'ⁿ'),
    ('i', 'ⁱ'),
    ('T', 'ᵀ'),
    ('′', '′'),
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
    ('-', '₋'),
    ('=', '₌'),
    ('(', '₍'),
    (')', '₎'),
    ('a', 'ₐ'),
    ('e', 'ₑ'),
    ('i', 'ᵢ'),
    ('j', 'ⱼ'),
    ('k', 'ₖ'),
    ('n', 'ₙ'),
    ('o', 'ₒ'),
    ('t', 'ₜ'),
    ('x', 'ₓ'),
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
