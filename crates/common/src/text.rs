//! Text pipeline: tokenize -> normalize -> stopwords -> stem.

use rust_stemmers::{Algorithm, Stemmer};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into", "is", "it",
    "no", "not", "of", "on", "or", "such", "that", "the", "their", "then", "there", "these",
    "they", "this", "to", "was", "will", "with",
];

/// A term and its position (token index) in the source text.
/// Positions count removed stopwords too, so phrase distances stay honest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub term: String,
    pub pos: u32,
}

pub struct Analyzer {
    stemmer: Stemmer,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer {
    pub fn new() -> Self {
        Self { stemmer: Stemmer::create(Algorithm::English) }
    }

    /// Lowercase, strip accents (NFD, drop combining marks).
    fn normalize(word: &str) -> String {
        word.nfd()
            .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
            .flat_map(|c| c.to_lowercase())
            .collect()
    }

    pub fn analyze(&self, text: &str) -> Vec<Token> {
        let mut out = Vec::new();
        for (pos, word) in text.unicode_words().enumerate() {
            let norm = Self::normalize(word);
            if norm.is_empty() || STOPWORDS.contains(&norm.as_str()) {
                continue;
            }
            let term = self.stemmer.stem(&norm).into_owned();
            out.push(Token { term, pos: pos as u32 });
        }
        out
    }

    /// Terms only, for queries.
    pub fn terms(&self, text: &str) -> Vec<String> {
        self.analyze(text).into_iter().map(|t| t.term).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_and_drops_stopwords() {
        let a = Analyzer::new();
        assert_eq!(a.terms("The runners are running"), vec!["runner", "run"]);
    }

    #[test]
    fn folds_accents_and_case() {
        let a = Analyzer::new();
        assert_eq!(a.terms("Café CAFÉ cafe"), vec!["cafe", "cafe", "cafe"]);
    }

    #[test]
    fn strips_punctuation() {
        let a = Analyzer::new();
        assert_eq!(a.terms("Hello, world!"), vec!["hello", "world"]);
    }

    #[test]
    fn positions_skip_over_stopwords() {
        let a = Analyzer::new();
        let t = a.analyze("rust is fast");
        assert_eq!(t.iter().map(|t| t.pos).collect::<Vec<_>>(), vec![0, 2]);
    }

    #[test]
    fn query_and_doc_agree() {
        let a = Analyzer::new();
        assert_eq!(a.terms("Searching engines"), a.terms("search engine"));
    }
}
