//! Estimated values this crate reads, one serde struct per register group (`quota`,
//! `seed`, `trust`, `ask`, `session`, `wrapup`). `Default` carries the register's
//! current values and is the only place they are written; use sites take the struct as
//! an argument.
