//! The mechanical layer's admission, journal and history, moved out of the add-in (John's
//! pattern: the exe owns validation, state and replies; the Python add-in only executes the
//! Fusion API work). Pure and OS-blind: SQLite and the dispatch files live in the bridge crate.

pub mod canonical;
pub mod history;
pub mod journal;
pub mod validate;
