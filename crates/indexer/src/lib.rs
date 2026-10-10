//! On-disk inverted index: builder and reader.
//!
//! Directory layout:
//!   meta.json      corpus stats + per-doc metadata (url, title, length, source offset)
//!   terms.bin      term dictionary: term -> (doc_freq, offset, byte_len) into postings.bin
//!   postings.bin   per-term posting lists, concatenated in term order
//!
//! One posting (all varints):
//!   doc_id delta | tf_body | tf_title | tf_body body-position deltas
//! Doc ids ascend within a list, so deltas stay tiny.

pub mod varint;

use anyhow::{Context, Result};
use common::{text::Analyzer, DocId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct DocMeta {
    pub url: String,
    pub title: String,
    /// Body token count (BM25 document length).
    pub len: u32,
    /// Byte range of the original JSON line in `Meta::source`, for snippets.
    pub offset: u64,
    pub bytes: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Meta {
    pub num_docs: u32,
    pub avg_doc_len: f32,
    /// Absolute path of the docs.jsonl this index was built from.
    pub source: String,
    pub docs: Vec<DocMeta>,
}

#[derive(Debug, Clone, Copy)]
pub struct TermInfo {
    pub doc_freq: u32,
    pub offset: u64,
    pub bytes: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Posting {
    pub doc: DocId,
    pub tf_body: u32,
    pub tf_title: u32,
    pub positions: Vec<u32>,
}

/// Per-term posting buffer, encoded incrementally as docs are added in id order.
#[derive(Default)]
struct TermBuf {
    doc_freq: u32,
    last_doc: DocId,
    buf: Vec<u8>,
}

#[derive(Default)]
pub struct BuildStats {
    pub docs: u32,
    pub skipped_duplicates: u32,
    pub terms: usize,
    pub postings: u64,
    pub postings_bytes: u64,
}

/// Build an index from a crawler `docs.jsonl` into `out_dir`.
pub fn build(docs_path: &Path, out_dir: &Path) -> Result<BuildStats> {
    std::fs::create_dir_all(out_dir)?;
    let analyzer = Analyzer::new();
    let mut terms: HashMap<String, TermBuf> = HashMap::new();
    let mut docs: Vec<DocMeta> = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();
    let mut stats = BuildStats::default();
    let mut total_len: u64 = 0;

    let mut reader = BufReader::new(File::open(docs_path).with_context(|| docs_path.display().to_string())?);
    let mut line = String::new();
    let mut offset: u64 = 0;
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        let this_offset = offset;
        offset += n as u64;
        if line.trim().is_empty() {
            continue;
        }
        // Crawler ids restart on every run, so assign our own and dedupe by URL.
        let d: common::Doc = match serde_json::from_str(&line) {
            Ok(d) => d,
            Err(_) => continue, // e.g. a line cut off by a killed crawler
        };
        if !seen_urls.insert(d.url.clone()) {
            stats.skipped_duplicates += 1;
            continue;
        }
        let id = docs.len() as DocId;

        let body = analyzer.analyze(&d.text);
        let title = analyzer.terms(&d.title);

        // term -> (body positions, title tf)
        let mut local: HashMap<&str, (Vec<u32>, u32)> = HashMap::new();
        for t in &body {
            local.entry(&t.term).or_default().0.push(t.pos);
        }
        for t in &title {
            local.entry(t).or_default().1 += 1;
        }
        for (term, (positions, tf_title)) in local {
            let tb = terms.entry(term.to_string()).or_default();
            varint::write(&mut tb.buf, (id - tb.last_doc) as u64);
            varint::write(&mut tb.buf, positions.len() as u64);
            varint::write(&mut tb.buf, tf_title as u64);
            let mut prev = 0;
            for p in positions {
                varint::write(&mut tb.buf, (p - prev) as u64);
                prev = p;
            }
            tb.last_doc = id;
            tb.doc_freq += 1;
            stats.postings += 1;
        }

        total_len += body.len() as u64;
        docs.push(DocMeta {
            url: d.url,
            title: d.title,
            len: body.len() as u32,
            offset: this_offset,
            bytes: n as u32,
        });
    }

    // Flush: terms in sorted order so the dictionary is deterministic.
    let mut sorted: Vec<(String, TermBuf)> = terms.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let mut postings = BufWriter::new(File::create(out_dir.join("postings.bin"))?);
    let mut dict = Vec::new();
    let mut off: u64 = 0;
    for (term, tb) in &sorted {
        postings.write_all(&tb.buf)?;
        varint::write(&mut dict, term.len() as u64);
        dict.extend_from_slice(term.as_bytes());
        varint::write(&mut dict, tb.doc_freq as u64);
        varint::write(&mut dict, off);
        varint::write(&mut dict, tb.buf.len() as u64);
        off += tb.buf.len() as u64;
    }
    postings.flush()?;
    std::fs::write(out_dir.join("terms.bin"), dict)?;

    let n = docs.len() as u32;
    let meta = Meta {
        num_docs: n,
        avg_doc_len: if n == 0 { 0.0 } else { total_len as f32 / n as f32 },
        source: std::fs::canonicalize(docs_path)?.to_string_lossy().into_owned(),
        docs,
    };
    serde_json::to_writer(BufWriter::new(File::create(out_dir.join("meta.json"))?), &meta)?;

    stats.docs = n;
    stats.terms = sorted.len();
    stats.postings_bytes = off;
    Ok(stats)
}

/// Read-only handle on a built index. The dictionary and doc metadata live in
/// RAM; postings are read from disk on demand.
pub struct Index {
    pub meta: Meta,
    terms: HashMap<String, TermInfo>,
    postings: File,
}

impl Index {
    pub fn open(dir: &Path) -> Result<Self> {
        let meta: Meta = serde_json::from_reader(BufReader::new(
            File::open(dir.join("meta.json")).context("meta.json (did you build the index?)")?,
        ))?;
        let raw = std::fs::read(dir.join("terms.bin"))?;
        let mut terms = HashMap::new();
        let mut pos = 0;
        while pos < raw.len() {
            let tl = varint::read(&raw, &mut pos) as usize;
            let term = String::from_utf8(raw[pos..pos + tl].to_vec())?;
            pos += tl;
            let doc_freq = varint::read(&raw, &mut pos) as u32;
            let offset = varint::read(&raw, &mut pos);
            let bytes = varint::read(&raw, &mut pos) as u32;
            terms.insert(term, TermInfo { doc_freq, offset, bytes });
        }
        Ok(Self { meta, terms, postings: File::open(dir.join("postings.bin"))? })
    }

    pub fn term_info(&self, term: &str) -> Option<TermInfo> {
        self.terms.get(term).copied()
    }

    pub fn num_terms(&self) -> usize {
        self.terms.len()
    }

    /// Decode the full posting list for a term (empty if unknown).
    pub fn postings(&self, term: &str) -> Result<Vec<Posting>> {
        let Some(info) = self.term_info(term) else { return Ok(vec![]) };
        let mut buf = vec![0u8; info.bytes as usize];
        self.postings.read_exact_at(&mut buf, info.offset)?;
        let mut out = Vec::with_capacity(info.doc_freq as usize);
        let (mut pos, mut doc) = (0usize, 0u32);
        while pos < buf.len() {
            doc += varint::read(&buf, &mut pos) as u32;
            let tf_body = varint::read(&buf, &mut pos) as u32;
            let tf_title = varint::read(&buf, &mut pos) as u32;
            let mut positions = Vec::with_capacity(tf_body as usize);
            let mut p = 0;
            for _ in 0..tf_body {
                p += varint::read(&buf, &mut pos) as u32;
                positions.push(p);
            }
            out.push(Posting { doc, tf_body, tf_title, positions });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("se-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_docs(dir: &Path, docs: &[(&str, &str, &str)]) -> std::path::PathBuf {
        let p = dir.join("docs.jsonl");
        let mut f = File::create(&p).unwrap();
        for (i, (url, title, text)) in docs.iter().enumerate() {
            let d = common::Doc { id: i as u32, url: url.to_string(), title: title.to_string(), text: text.to_string() };
            writeln!(f, "{}", serde_json::to_string(&d).unwrap()).unwrap();
        }
        p
    }

    #[test]
    fn build_and_read_back() {
        let dir = tmpdir("rt");
        let docs = write_docs(
            &dir,
            &[
                ("https://x/a", "Rust language", "Rust is fast. Rust is safe and fast."),
                ("https://x/b", "Cooking", "Fast food is not healthy"),
                ("https://x/c", "Rust", "Nothing relevant here"),
            ],
        );
        let stats = build(&docs, &dir.join("idx")).unwrap();
        assert_eq!(stats.docs, 3);

        let idx = Index::open(&dir.join("idx")).unwrap();
        assert_eq!(idx.meta.num_docs, 3);

        // "rust": body tf 2 in doc 0 (positions 0 and 3), title hit in docs 0 and 2.
        let rust = idx.postings("rust").unwrap();
        assert_eq!(rust.len(), 2 + 0); // docs 0 and 2 only
        assert_eq!(rust[0], Posting { doc: 0, tf_body: 2, tf_title: 1, positions: vec![0, 3] });
        assert_eq!(rust[1], Posting { doc: 2, tf_body: 0, tf_title: 1, positions: vec![] });

        // "fast" appears in docs 0 (twice) and 1 (once); doc_freq = 2.
        let fast = idx.postings("fast").unwrap();
        assert_eq!(fast.iter().map(|p| (p.doc, p.tf_body)).collect::<Vec<_>>(), vec![(0, 2), (1, 1)]);
        assert_eq!(idx.term_info("fast").unwrap().doc_freq, 2);

        // stopwords never indexed, unknown terms empty.
        assert!(idx.term_info("is").is_none());
        assert!(idx.postings("zzz").unwrap().is_empty());
    }

    #[test]
    fn dedupes_urls_and_tolerates_truncated_line() {
        let dir = tmpdir("dup");
        let p = write_docs(&dir, &[("https://x/a", "A", "alpha beta"), ("https://x/a", "A", "alpha beta")]);
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        write!(f, "{{\"id\":9,\"url\":\"https://x/z\",\"ti").unwrap();
        let stats = build(&p, &dir.join("idx")).unwrap();
        assert_eq!((stats.docs, stats.skipped_duplicates), (1, 1));
    }

    #[test]
    fn doc_offsets_point_at_source_lines() {
        let dir = tmpdir("off");
        let p = write_docs(&dir, &[("https://x/a", "A", "one two"), ("https://x/b", "B", "three four")]);
        build(&p, &dir.join("idx")).unwrap();
        let idx = Index::open(&dir.join("idx")).unwrap();
        let raw = std::fs::read(&p).unwrap();
        let m = &idx.meta.docs[1];
        let slice = &raw[m.offset as usize..(m.offset + m.bytes as u64) as usize];
        let d: common::Doc = serde_json::from_slice(slice).unwrap();
        assert_eq!(d.url, "https://x/b");
    }
}
