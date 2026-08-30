/// One retrieval unit, as stored.
///
/// `span_of` / `span_from` / `span_to` locate it inside its source sequence. Units do not
/// overlap, so these are not for deduplication — they are what lets a ranked hit be
/// widened to its neighbours, which is how adjacent context is recovered without storing
/// it twice.
#[derive(Debug, Clone)]
pub struct Unit {
    pub id: String,
    /// Dense index into the vector store. Also the FTS rowid.
    pub ord: i64,
    pub template: String,
    /// Present when the unit belongs to a person rather than merely mentioning one.
    pub person: Option<String>,
    pub page: String,
    /// Source revision. Every unit has one; it is what makes redistribution traceable.
    pub revid: Option<i64>,
    pub title: String,
    /// Context line prepended when embedding. Part of what was indexed, so it is part of
    /// what a citation should show.
    pub header: String,
    pub text: String,
    pub chars: i64,
    pub span_of: Option<String>,
    pub span_from: Option<i64>,
    pub span_to: Option<i64>,
}

impl Unit {
    /// What the encoder saw. Reproduces the build-time string exactly, because anything
    /// else would embed a query against text that was never indexed.
    pub fn embed_text(&self) -> String {
        if self.header.is_empty() {
            self.text.clone()
        } else {
            format!("{}\n{}", self.header, self.text)
        }
    }

    /// Does this unit sit next to `other` in the same source sequence?
    pub fn adjacent_to(&self, other: &Unit) -> bool {
        match (
            &self.span_of,
            &other.span_of,
            self.span_to,
            other.span_from,
            self.span_from,
            other.span_to,
        ) {
            (Some(a), Some(b), Some(a_to), Some(b_from), Some(a_from), Some(b_to)) if a == b => {
                a_to + 1 == b_from || b_to + 1 == a_from
            }
            _ => false,
        }
    }
}

/// Per-template size statistics, published so the ranker can calibrate.
///
/// Lexical and dense scores are not comparable across unit types whose lengths differ by
/// an order of magnitude — measured: a voice line's body is 22 characters at the median,
/// a profile's 95th percentile is 720. Without these the ranker would have to guess.
#[derive(Debug, Clone)]
pub struct TemplateStats {
    pub template: String,
    pub count: i64,
    pub embed_p50: i64,
    pub embed_p95: i64,
    pub embed_max: i64,
}

/// A person on the roster.
#[derive(Debug, Clone)]
pub struct Person {
    pub person_id: String,
    pub primary_page: String,
    pub display: String,
    /// `[{page, kind}]`, canonical form first.
    pub forms: Vec<PersonForm>,
    /// Source material in characters. Drives `confidence`.
    pub material: i64,
    /// Normalised material volume, not a quality judgement. Used to *change behaviour*
    /// rather than to apologise: a thinly covered person should be written as terse and
    /// unwilling to speculate, which is a characterisation.
    pub confidence: f64,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PersonForm {
    pub page: String,
    pub kind: String,
}
