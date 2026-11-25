/// `text` with every run of white space, line breaks among them, a single space.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
