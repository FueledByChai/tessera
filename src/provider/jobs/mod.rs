//! The provider download jobs (decisions 0020 and 0022), written against the [`Provider`]
//! trait alone: a job knows a dataset (a folder, a listing, a from-date) and a budget, and
//! nothing about the service's catalog or its tables. The service maps its rows onto a job's
//! input and records the outcome; an integration test drives a job over a stub server and a
//! temporary folder the same way.
//!
//! [`eod`] is the daily-bar job (DS-08); [`intraday`] extends the intraday files (DS-09).
//! Both share the record types in [`eod`] (the state, the progress, a skipped symbol) and
//! its part-then-rename write.
//!
//! [`Provider`]: super::Provider

pub mod eod;
pub mod intraday;
