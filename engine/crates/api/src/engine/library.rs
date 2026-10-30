//! The corpus as a library: search, passages and citation resolution. Every corpus text
//! passes through the user-name substitution on its way out.

use super::Engine;
use crate::convert;
use crate::corpus::{self, Corpus};
use crate::display::Namer;
use crate::error::{ApiError, ApiResult};
use crate::types::*;
use crate::views::*;

/// Hits the library asks the index for; the screen shows them a page at a time.
const SEARCH_WINDOW: usize = 90;

fn nonempty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

impl Engine {
    /// Check the file as a corpus, remember it for the next start and ask the front end to
    /// open the home again on it.
    pub(crate) fn set_corpus_impl(&self, path: &str) -> ApiResult<()> {
        let path = corpus::choose(path)?;
        self.home.set_chosen_corpus(&path).map_err(super::io)?;
        (self.ports.restart)();
        Ok(())
    }

    /// Hand a web address to the system browser. Only plain http(s) addresses go out: a
    /// webview opens nothing on its own, and the opener must never be given a path, an
    /// option or another scheme.
    pub(crate) fn open_external_impl(&self, url: &str) -> ApiResult<()> {
        let url = url.trim();
        let web = url.starts_with("https://") || url.starts_with("http://");
        if !web || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(ApiError::Invalid {
                field: "url".into(),
            });
        }
        let open = self
            .ports
            .open_url
            .clone()
            .ok_or(ApiError::NotImplemented)?;
        open(url);
        Ok(())
    }

    fn passage(&self, c: &Corpus, namer: &Namer, u: &folio::Unit) -> Passage {
        Passage {
            citation: convert::unit_citation(u),
            template: u.template.clone(),
            header: namer.show(&u.header),
            text: namer.show(&u.text),
            persons: u.persons.iter().map(|p| self.person_ref(p)).collect(),
            source_url: nonempty(c.manifest.source_url(&u.page)),
        }
    }

    pub(crate) fn search_impl(
        &self,
        query: &str,
        persons: &[PersonId],
        templates: &[String],
    ) -> ApiResult<SearchResults> {
        let c = self.corpus()?;
        if query.trim().is_empty() {
            return Ok(SearchResults {
                hits: Vec::new(),
                confidence: Confidence::Low,
                understood_as: None,
            });
        }
        let namer = self.namer()?;
        let mut request = index::SearchRequest::new(query);
        request.top_k = SEARCH_WINDOW;
        request.filters.persons = persons.iter().map(|p| p.0.clone()).collect();
        request.filters.templates = templates.to_vec();
        let found = c.with_index(|ix| ix.search(&request))?;
        Ok(SearchResults {
            hits: found
                .hits
                .iter()
                .filter_map(|h| h.item.unit())
                .map(|u| self.passage(c, &namer, u))
                .collect(),
            confidence: convert::confidence(found.confidence.level),
            understood_as: found.resolved.map(|r| namer.show(&r.target)),
        })
    }

    pub(crate) fn read_unit_impl(&self, id: &str, around: u32) -> ApiResult<UnitView> {
        let c = self.corpus()?;
        let namer = self.namer()?;
        let found = c.with_index(|ix| ix.get(id, i64::from(around)))?;
        let Some((unit, near)) = found else {
            return Err(ApiError::NotFound);
        };
        // Neighbours come in source order; the ones that start before the unit precede it.
        let at = unit.span_from.unwrap_or(0);
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for n in &near {
            let p = self.passage(c, &namer, n);
            if n.span_from.unwrap_or(0) < at {
                before.push(p);
            } else {
                after.push(p);
            }
        }
        Ok(UnitView {
            unit: self.passage(c, &namer, &unit),
            before,
            after,
        })
    }

    pub(crate) fn templates_impl(&self) -> ApiResult<Vec<String>> {
        let c = self.corpus()?;
        let mut all: Vec<String> = c.with_folio(|f| {
            let mut stmt = f
                .conn()
                .prepare_cached("SELECT DISTINCT template FROM chunks")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })?;
        all.sort();
        Ok(all)
    }

    /// Three levels: the chunk id still exists; else the unit of the same page whose span
    /// overlaps the cited one most; else only the stored links remain.
    pub(crate) fn resolve_impl(&self, cite: &Citation) -> ApiResult<CitationView> {
        let c = self.corpus()?;
        let namer = self.namer()?;
        let source_url = nonempty(c.manifest.source_url(&cite.page));
        let revision_url = nonempty(c.revision_url(&cite.page, cite.revid));
        let view = |status, unit: Option<folio::Unit>| CitationView {
            status,
            passage: unit.map(|u| self.passage(c, &namer, &u)),
            source_url: source_url.clone(),
            revision_url: revision_url.clone(),
        };

        if let Some(u) = c.with_folio(|f| f.unit_by_id(&cite.chunk_id))? {
            return Ok(view(CitationStatus::Current, Some(u)));
        }
        let (from, to) = (i64::from(cite.span_from), i64::from(cite.span_to));
        let best = c.with_folio(|f| {
            let mut stmt = f.conn().prepare_cached(
                "SELECT id, span_from, span_to FROM chunks
                 WHERE page = ?1 AND span_from <= ?3 AND span_to >= ?2 ORDER BY ord",
            )?;
            let rows = stmt.query_map(rusqlite::params![cite.page, from, to], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?;
            let mut best: Option<(i64, String)> = None;
            for row in rows {
                let (id, a, b) = row?;
                let overlap = b.min(to) - a.max(from) + 1;
                if best.as_ref().is_none_or(|(o, _)| overlap > *o) {
                    best = Some((overlap, id));
                }
            }
            match best {
                Some((_, id)) => f.unit_by_id(&id),
                None => Ok(None),
            }
        })?;
        Ok(match best {
            Some(u) => view(CitationStatus::Updated, Some(u)),
            None => view(CitationStatus::Gone, None),
        })
    }
}
