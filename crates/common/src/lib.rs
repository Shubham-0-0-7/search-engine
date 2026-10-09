//! Shared types and the text pipeline. The crawler, indexer and query path
//! must all go through this so terms are normalized identically.

pub mod text;
pub mod url_norm;

use serde::{Deserialize, Serialize};

pub type DocId = u32;

/// A cleaned page as stored by the crawler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Doc {
    pub id: DocId,
    pub url: String,
    pub title: String,
    pub text: String,
}
