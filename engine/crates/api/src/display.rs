//! The one place corpus text is readied for display.
//!
//! The corpus stores the user's name as a placeholder (manifest `user_placeholder`) so the
//! same file is right for every user. Everything the UI shows that came out of the folio
//! (passages, citations, reasons, summaries) passes through [`Namer::show`], which puts in
//! the user's name, or the pack's word for "you" (wording `user.you`) when no name is
//! set. The folio itself is never written.

/// Substitutes the user-name placeholder. Cheap to build per command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namer {
    placeholder: String,
    name: String,
}

/// The wording key used when the user has not given a name.
pub const USER_YOU: &str = "user.you";
/// Neutral fallback when the pack has no `user.you` either.
const YOU: &str = "you";

impl Namer {
    pub fn new(placeholder: &str, user_name: Option<&str>, you: Option<&str>) -> Self {
        let name = user_name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .or(you)
            .unwrap_or(YOU);
        Self {
            placeholder: placeholder.to_owned(),
            name: name.to_owned(),
        }
    }

    /// `text` with every placeholder replaced; unchanged when the corpus declares none.
    pub fn show(&self, text: &str) -> String {
        if self.placeholder.is_empty() {
            text.to_owned()
        } else {
            text.replace(&self.placeholder, &self.name)
        }
    }

    pub fn show_opt(&self, text: Option<&str>) -> Option<String> {
        text.map(|t| self.show(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_then_you_then_neutral() {
        let named = Namer::new("<U>", Some(" Ada "), Some("Doctor"));
        assert_eq!(named.show("Dr.<U>, <U>!"), "Dr.Ada, Ada!");
        let unnamed = Namer::new("<U>", None, Some("Doctor"));
        assert_eq!(unnamed.show("Dr.<U>"), "Dr.Doctor");
        assert_eq!(Namer::new("<U>", Some("  "), None).show("<U>"), "you");
        // No placeholder declared: text passes through, even text that looks like one.
        assert_eq!(Namer::new("", Some("Ada"), None).show("<U> and <>"), "<U> and <>");
        assert_eq!(named.show_opt(None), None);
    }
}
