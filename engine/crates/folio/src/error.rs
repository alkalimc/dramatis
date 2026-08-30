use std::path::PathBuf;

/// Everything that can go wrong opening or reading a corpus.
///
/// The two interesting variants are `UnknownFormat` and `UnmetRequirement`, because they
/// exist to make the reader **refuse** rather than degrade. A corpus is a file someone
/// downloaded; the failure mode worth engineering against is not a crash but a silent
/// half-working state that produces slightly wrong answers forever.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no corpus at {0}")]
    Missing(PathBuf),

    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("malformed manifest key {key}: {source}")]
    Manifest {
        key: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("manifest is missing required key {0}")]
    MissingKey(&'static str),

    /// The corpus was written by a newer forge. Refusing is correct: a format bump means
    /// a column changed meaning, and guessing at the new meaning is how a reader starts
    /// answering questions wrongly without anyone noticing.
    #[error(
        "corpus format version {found} is not supported (this build reads {supported}); \
         upgrade the engine rather than this corpus"
    )]
    UnknownFormat { found: i64, supported: i64 },

    /// The corpus declares an obligation this build does not implement.
    ///
    /// Units are stored once, so `neighbor_expand` means "widen a ranked hit to its
    /// neighbours or you will serve truncated context". A reader that ignores that
    /// returns answers which look complete and are not — the exact class of failure the
    /// declaration exists to prevent, so it is fatal rather than a warning.
    #[error("corpus requires capability {requirement:?}, which this build does not implement")]
    UnmetRequirement { requirement: String },

    #[error("vector store is {actual} bytes, but {count} units x {dim} dims x 2 needs {expected}")]
    VectorSizeMismatch {
        actual: usize,
        expected: usize,
        count: usize,
        dim: usize,
    },

    #[error("query dimension {query} does not match corpus dimension {corpus}")]
    DimMismatch { query: usize, corpus: usize },
}

pub type Result<T> = std::result::Result<T, Error>;
