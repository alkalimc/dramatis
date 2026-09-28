//! Scoring the retriever against the structural benchmark.
//!
//! The suite was built from the corpus's own structure — redirects, real names, alternate
//! forms, heading paths, dialogue circumstances — so every judgement traces to something the
//! source declares rather than to anyone's opinion about relevance. That is what makes
//! tens of thousands of graded queries affordable, and also what bounds the claim: this
//! measures whether retrieval recovers relations the source already states, not whether an
//! answer satisfies a reader.
//!
//! The harness refuses to produce a single headline number. Families of wildly unequal
//! size test different claims, and two strata differ in whether exact string matching
//! solves them, so a mean over all of it would be a number with no referent. Every entry
//! point here reports per family and per stratum; the only aggregate offered is a
//! macro-average, which is a summary, not a result.

pub mod calibrate;
pub mod error;
pub mod metrics;
pub mod report;
pub mod runner;
pub mod suite;

pub use error::{Error, Result};
pub use metrics::{Aggregate, Scored};
pub use runner::{Config, Outcome};
pub use suite::{Query, Suite};
