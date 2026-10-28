//! Asking the person running `kkt setup`. Every question has a flag that
//! answers it instead, so setup also runs with no one to ask.

/// A question, and the flag that answers it without asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Question<'a> {
    pub text: &'a str,
    /// Such as `--mode`.
    pub flag: &'static str,
}

/// Asks questions. The terminal implementation is in `main.rs`; tests use
/// scripted answers.
pub trait Ask {
    /// The index of the option chosen from `options`.
    fn choose(&self, question: Question, options: &[&str]) -> Result<usize, String>;
    /// Yes or no.
    fn confirm(&self, question: Question) -> Result<bool, String>;
    /// A line of text.
    fn text(&self, question: Question) -> Result<String, String>;
}

/// For runs with no terminal: every question is an error naming the flag
/// that answers it.
pub struct Unattended;

impl Unattended {
    fn refuse(question: Question) -> String {
        format!(
            "{} No terminal to ask on: pass {}.",
            question.text, question.flag
        )
    }
}

impl Ask for Unattended {
    fn choose(&self, question: Question, _: &[&str]) -> Result<usize, String> {
        Err(Unattended::refuse(question))
    }

    fn confirm(&self, question: Question) -> Result<bool, String> {
        Err(Unattended::refuse(question))
    }

    fn text(&self, question: Question) -> Result<String, String> {
        Err(Unattended::refuse(question))
    }
}
