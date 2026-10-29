//! Talking with the person running `kkt setup`. Every question has a flag
//! that answers it instead, so setup also runs with no one to ask.

/// A question, and the flag that answers it without asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Question<'a> {
    pub text: &'a str,
    /// Such as `--mode`.
    pub flag: &'static str,
    /// The answer Enter accepts for a yes/no question at a terminal.
    /// This never answers a question when there is no terminal.
    pub default: Option<bool>,
}

/// Asks questions and says what happened. The terminal implementation is in
/// `main.rs`; tests use scripted answers.
pub trait Ask {
    /// The index of the option chosen from `options`. Enter accepts `default`
    /// at a terminal; without a terminal the question still needs its flag.
    fn choose(&self, question: Question, options: &[&str], default: usize)
    -> Result<usize, String>;
    /// Yes or no.
    fn confirm(&self, question: Question) -> Result<bool, String>;
    /// A line of text.
    fn text(&self, question: Question) -> Result<String, String>;
    /// Says something, such as what was just done.
    fn tell(&self, text: &str);
}

/// The error for a question there is no one to ask: it names the flag that
/// answers it.
pub fn unanswered(question: Question) -> String {
    format!(
        "{} No terminal to ask on: pass {}.",
        question.text, question.flag
    )
}
