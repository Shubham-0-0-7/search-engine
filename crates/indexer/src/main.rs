use anyhow::Result;
use indexer::{build, Index};
use std::path::PathBuf;
use std::time::Instant;

fn usage() -> ! {
    eprintln!(
        "usage:\n  indexer build [--docs data/docs.jsonl] [--out data/index]\n  indexer show <term> [--index data/index]   # decode a term's postings (analyzes the term first)"
    );
    std::process::exit(2)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else { usage() };
    let (mut docs, mut out, mut positional) = (PathBuf::from("data/docs.jsonl"), PathBuf::from("data/index"), None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--docs" => docs = args.next().unwrap_or_else(|| usage()).into(),
            "--out" | "--index" => out = args.next().unwrap_or_else(|| usage()).into(),
            s => positional = Some(s.to_string()),
        }
    }

    match cmd.as_str() {
        "build" => {
            let t = Instant::now();
            let s = build(&docs, &out)?;
            println!(
                "indexed {} docs ({} duplicate urls skipped)\n  {} unique terms, {} postings, postings.bin = {:.1} MB\n  took {:.2?} -> {}",
                s.docs, s.skipped_duplicates, s.terms, s.postings,
                s.postings_bytes as f64 / 1e6, t.elapsed(), out.display()
            );
        }
        "show" => {
            let word = positional.unwrap_or_else(|| usage());
            let idx = Index::open(&out)?;
            let analyzer = common::text::Analyzer::new();
            let terms = analyzer.terms(&word);
            let Some(term) = terms.first() else { println!("'{word}' is a stopword / empty after analysis"); return Ok(()) };
            let Some(info) = idx.term_info(term) else { println!("term '{term}' not in index"); return Ok(()) };
            println!("'{word}' -> term '{term}': doc_freq={} ({} bytes), N={}, avgdl={:.0}",
                info.doc_freq, info.bytes, idx.meta.num_docs, idx.meta.avg_doc_len);
            for p in idx.postings(term)?.iter().take(10) {
                let m = &idx.meta.docs[p.doc as usize];
                println!("  doc {:>5} tf_body={:<3} tf_title={} len={:<6} {}  pos={:?}",
                    p.doc, p.tf_body, p.tf_title, m.len, m.title, &p.positions[..p.positions.len().min(6)]);
            }
        }
        _ => usage(),
    }
    Ok(())
}
