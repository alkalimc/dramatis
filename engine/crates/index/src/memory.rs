//! Scoring caller-supplied memories on the corpus's BM25 scale.
//!
//! A memory is not in the index, so FTS5 cannot score it. It is scored here with the
//! formula FTS5 uses (k1 = 1.2, b = 0.75, IDF floored at 1e-6), against the corpus's row
//! count, average length and per-term document frequency, as if it were one more unit.
//! That keeps a memory and a unit comparable without inventing a conversion between two
//! scales.

use rusqlite::Connection;

use crate::Result;
use crate::segment::Segmenter;

const K1: f64 = 1.2;
const B: f64 = 0.75;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CorpusStats {
    rows: f64,
    avg_len: f64,
}

impl CorpusStats {
    /// Row count and total token count live in the FTS5 averages record (rowid 1 of the
    /// `_data` shadow table): a varint row count, then one varint token total per column.
    /// Reading it directly avoids a scan of the whole index.
    pub(crate) fn read(conn: &Connection) -> Result<Self> {
        let block: Option<Vec<u8>> = conn
            .query_row(
                "SELECT block FROM chunks_fts_data WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .ok();
        if let Some(block) = block
            && let Some((rows, at)) = varint(&block, 0)
            && let Some((tokens, _)) = varint(&block, at)
            && rows > 0
        {
            return Ok(Self {
                rows: rows as f64,
                avg_len: tokens as f64 / rows as f64,
            });
        }
        // An empty or unreadable averages record: fall back to the unit count and a length
        // that makes the length normalisation neutral.
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        Ok(Self {
            rows: rows.max(1) as f64,
            avg_len: 0.0,
        })
    }
}

/// SQLite's varint: big-endian groups of seven bits, high bit set on all but the last,
/// with a ninth byte contributing all eight bits.
fn varint(buf: &[u8], mut at: usize) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    for _ in 0..8 {
        let byte = *buf.get(at)?;
        at += 1;
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte < 0x80 {
            return Some((value, at));
        }
    }
    let byte = *buf.get(at)?;
    Some(((value << 8) | u64::from(byte), at + 1))
}

/// Units whose index holds `term`. Quoted as a phrase exactly like the query side.
pub(crate) fn document_frequency(conn: &Connection, term: &str) -> Result<i64> {
    let phrase = format!("\"{}\"", term.replace('"', "\"\""));
    let mut stmt =
        conn.prepare_cached("SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH ?1")?;
    Ok(stmt.query_row([phrase], |row| row.get(0))?)
}

/// Query terms as the lexical path sends them: stopwords dropped, duplicates kept (FTS5
/// scores each OR-ed phrase separately), folded to lower case like the index tokenizer.
pub(crate) fn terms(segmenter: &Segmenter, query: &str) -> Vec<String> {
    segmenter
        .segment_for_query(query)
        .into_iter()
        .map(|t| t.to_lowercase())
        .collect()
}

/// BM25 of `text` for `terms`, on the corpus's statistics.
pub(crate) fn bm25(
    segmenter: &Segmenter,
    stats: &CorpusStats,
    terms: &[String],
    text: &str,
    mut df: impl FnMut(&str) -> Result<i64>,
) -> Result<f64> {
    if terms.is_empty() {
        return Ok(0.0);
    }
    // Index-side segmentation: the memory is scored as a unit would have been indexed.
    let tokens: Vec<String> = segmenter
        .segment(text)
        .into_iter()
        .map(|t| t.to_lowercase())
        .collect();
    let len = tokens.len() as f64;
    let avg_len = if stats.avg_len > 0.0 {
        stats.avg_len
    } else {
        len.max(1.0)
    };
    let mut score = 0.0;
    for term in terms {
        let freq = tokens.iter().filter(|t| *t == term).count() as f64;
        if freq == 0.0 {
            continue;
        }
        let hits = df(term)? as f64;
        let idf = ((stats.rows - hits + 0.5) / (hits + 0.5)).ln();
        let idf = if idf <= 0.0 { 1e-6 } else { idf };
        score += idf * (freq * (K1 + 1.0)) / (freq + K1 * (1.0 - B + B * len / avg_len));
    }
    Ok(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_decode_like_sqlite() {
        assert_eq!(varint(&[0x05], 0), Some((5, 1)));
        assert_eq!(varint(&[0x81, 0x00], 0), Some((128, 2)));
        assert_eq!(varint(&[0x83, 0xcd, 0x53], 0), Some((59123, 3)));
        assert_eq!(varint(&[0x81], 0), None, "truncated input");
    }

    #[test]
    fn a_rarer_term_scores_higher() {
        let seg = Segmenter::new(crate::segment::Kind::CharBigram, Vec::new());
        let stats = CorpusStats {
            rows: 1000.0,
            avg_len: 4.0,
        };
        let df = |t: &str| Ok(if t == "rare" { 2 } else { 400 });
        let rare = bm25(&seg, &stats, &["rare".into()], "a rare word", df).unwrap();
        let common = bm25(&seg, &stats, &["common".into()], "a common word", df).unwrap();
        assert!(rare > common && common > 0.0);
        assert_eq!(bm25(&seg, &stats, &["absent".into()], "a word", df).unwrap(), 0.0);
    }
}
