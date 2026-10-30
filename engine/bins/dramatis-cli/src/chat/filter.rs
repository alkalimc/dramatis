//! The words `/list` takes besides a name fragment: a mode, talked or never, persona.

use api::types::Mode;

/// Every filter word, in the order Tab offers them.
pub const WORDS: &[&str] = &[
    "all", "enabled", "frozen", "disabled", "talked", "never", "persona",
];

/// What `/list` shows: the filter words found in its argument, and the rest as a fragment
/// of a name. Mode words add up (`enabled frozen` is either); `talked` with `never`, or
/// neither, is everyone.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ListFilter {
    modes: Vec<Mode>,
    talked: bool,
    never: bool,
    persona: bool,
    /// The words that were not filter words, lowercased.
    pub text: Option<String>,
}

impl ListFilter {
    pub fn parse(args: Option<&str>) -> Self {
        let mut f = Self::default();
        let mut rest = Vec::new();
        for word in args.unwrap_or("").split_whitespace() {
            match word.to_lowercase().as_str() {
                "all" => {}
                "enabled" => f.modes.push(Mode::Enabled),
                "frozen" => f.modes.push(Mode::Frozen),
                "disabled" => f.modes.push(Mode::Disabled),
                "talked" => f.talked = true,
                "never" => f.never = true,
                "persona" => f.persona = true,
                _ => rest.push(word.to_lowercase()),
            }
        }
        f.text = (!rest.is_empty()).then(|| rest.join(" "));
        f
    }

    pub fn allows(&self, mode: Mode, talked: bool, persona: bool) -> bool {
        (self.modes.is_empty() || self.modes.contains(&mode))
            && (self.talked == self.never || talked == self.talked)
            && (!self.persona || persona)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> ListFilter {
        ListFilter::parse(Some(s))
    }

    #[test]
    fn filter_words_are_taken_out_and_the_rest_is_the_name_fragment() {
        let f = parse("Enabled an lee");
        assert_eq!(f.text.as_deref(), Some("an lee"));
        assert!(f.allows(Mode::Enabled, false, true) && !f.allows(Mode::Frozen, false, true));
        assert_eq!(ListFilter::parse(None), ListFilter::default());
        assert_eq!(parse("all"), ListFilter::default());
    }

    #[test]
    fn modes_add_up_and_talked_with_never_is_everyone() {
        let f = parse("enabled frozen");
        assert!(f.allows(Mode::Frozen, true, true) && !f.allows(Mode::Disabled, true, true));
        let talked = parse("talked");
        assert!(
            talked.allows(Mode::Frozen, true, true) && !talked.allows(Mode::Frozen, false, true)
        );
        let never = parse("never");
        assert!(never.allows(Mode::Frozen, false, true) && !never.allows(Mode::Frozen, true, true));
        assert!(parse("talked never").allows(Mode::Frozen, false, true));
        let both = parse("disabled talked");
        assert!(
            both.allows(Mode::Disabled, true, true) && !both.allows(Mode::Disabled, false, true)
        );
    }

    #[test]
    fn persona_keeps_only_those_who_have_one() {
        let f = parse("persona");
        assert!(f.allows(Mode::Frozen, false, true) && !f.allows(Mode::Frozen, false, false));
    }
}
