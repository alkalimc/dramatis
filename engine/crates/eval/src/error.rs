use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("suite line {line}: {source}")]
    Parse {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Index(#[from] index::Error),
    #[error(transparent)]
    Folio(#[from] folio::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
