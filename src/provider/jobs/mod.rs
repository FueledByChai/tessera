//! The provider download jobs (decisions 0020 and 0022), written against the [`Provider`]
//! trait alone: a job knows a dataset (a folder, a listing, a from-date) and a budget, and
//! nothing about the service's catalog or its tables. The service maps its rows onto a job's
//! input and records the outcome; an integration test drives a job over a stub server and a
//! temporary folder the same way.
//!
//! [`eod`] is the daily-bar job (DS-08); the intraday job joins it with DS-09.
//!
//! [`Provider`]: super::Provider

pub mod eod;
