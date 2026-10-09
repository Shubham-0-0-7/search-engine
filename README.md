# search-engine

A working search engine over a bounded Wikipedia corpus, in Rust.
Three components: **crawler → indexer → ranker**, exposed by a search API.

Goal: crawl a bounded slice of Wikipedia (~100k pages), build an on-disk
inverted index, and serve ranked results with BM25 in a few milliseconds.

---

## Scope & constraints (read first)

- **Bounded corpus, not "the whole web."** Start with English Wikipedia,
  seeded from a few topic hubs, capped at a page count (e.g. 100k). This keeps
  storage honest (~a few GB of text + index) and lets every stage actually finish.
- Single machine. ~36 GB free disk is the real budget — plan around it.
- Baseline success = crawler + inverted index + BM25 search API. PageRank and
  an ML re-ranker are **stretch goals**, not the baseline.

### Storage budget (rough)
- 100k pages × ~5 KB cleaned text ≈ 500 MB raw text.
- Inverted index ≈ 20–40% of text size ≈ 150–250 MB.
- Easily fits. If you later raise the cap, text is the thing that grows — store
  cleaned text compressed (zstd), not raw HTML.

---

## Architecture

```
          ┌──────────┐     ┌───────────┐     ┌──────────────┐
seeds ──▶ │ crawler  │ ──▶ │  indexer  │ ──▶ │ inverted idx │
          └──────────┘     └───────────┘     └──────┬───────┘
             │  fetched docs (cleaned text)         │
             ▼                                        ▼
        doc store (zstd)                      ┌──────────────┐
                                              │ ranker (BM25)│ ◀── query
                                              └──────┬───────┘
                                                     ▼
                                              axum HTTP API ──▶ JSON results
```

---

## 1. Crawler

**Model:** BFS over a work queue, one worker per host, politeness-first.

- **Frontier:** a queue of normalized URLs to visit. FIFO to start.
  *(Not PageRank — PageRank needs the link graph you don't have until after
  you crawl. Add it later as a re-crawl signal.)*
- **Host sharding for politeness, not just load:** group URLs by host; at most
  one in-flight request per host, with a delay (≥1 s/host). Respect
  `robots.txt` and `Crawl-delay`. This is what keeps Wikipedia from banning you.
- **Dedup, two kinds:**
  - *URL-seen:* normalized-URL set (strip fragments, sort query params,
    lowercase host). Use a `HashSet`; swap to a Bloom filter only if memory bites.
  - *Content-seen:* hash of the **cleaned text** (not raw HTML — same article
    renders different HTML). SHA-256 to start; SimHash later for near-dupes.
- **Per-page flow:** fetch → check content-type → extract text + outlinks →
  content-seen check → store doc → enqueue new outlinks (same-domain only, to
  stay bounded).
- **Checkpointing:** persist frontier + seen-sets periodically so a crash
  doesn't restart the crawl from zero.

**Crates:** `reqwest` (HTTP), `tokio` (async workers), `scraper` or `lol_html`
(HTML parsing), `url` (normalization), `texting_robots` (robots.txt),
`governor` (rate limiting), `sha2`.

**Output:** a doc store — one record per page: `{doc_id, url, title, text}`,
written as zstd-compressed JSONL or into `sled`/`redb`.

---

## 2. Indexer

Turns cleaned text into an **inverted index**: `term → postings`.

**Pipeline (order matters):**
1. **Strip tags / take main content** — done in the crawler already; here you
   work from cleaned text + keep the title (title hits should score higher).
2. **Tokenize** — split into words on Unicode boundaries.
3. **Normalize** — lowercase, strip punctuation, fold accents.
4. **Stem** — `running → run`. Porter stemmer (`rust-stemmers`).
5. **Stopword removal** — drop `the`, `is`, `at`, … (small static list).
6. **Build postings** — for each term, a list of
   `(doc_id, term_frequency, positions[])`. Positions enable phrase search later.

**What your draft was missing — store these, BM25 needs them:**
- `doc_length` per document (number of tokens).
- `avg_doc_length` across the corpus.
- `doc_frequency` per term (how many docs contain it).
- total `doc_count`.

**On-disk format (start simple):**
- Term dictionary: sorted `term → (doc_freq, offset into postings file)`.
- Postings file: concatenated postings, delta-encoded doc_ids (varint) to save
  space. Read a term's postings by seeking to its offset.
- A separate doc-meta file: `doc_id → (url, title, doc_length)`.

Build in memory in segments, flush each segment to disk, then merge segments
(this is how Lucene does it). For v1, if the whole index fits in RAM, build it
all at once and serialize — don't over-engineer the merge yet.

**Shortcut option:** the `tantivy` crate *is* a Rust Lucene — if the goal were
"have search," you'd use it in an afternoon. Since the goal is **understanding**,
build the inverted index yourself; keep `tantivy` in your back pocket as a
reference/fallback.

---

## 3. Ranker

**Baseline: BM25 over the inverted index.** This is the whole project's payoff
and is genuinely good — don't skip it for ML.

For a query:
1. Tokenize/normalize/stem the query with the **same** pipeline as indexing.
2. For each query term, load its postings.
3. Score each candidate doc with BM25:
   ```
   score(d,q) = Σ_term IDF(term) · tf·(k1+1) / (tf + k1·(1 - b + b·|d|/avgdl))
   IDF(term)  = ln((N - df + 0.5)/(df + 0.5) + 1)
   ```
   with `k1 ≈ 1.2`, `b ≈ 0.75`.
4. Boost title matches (index title tokens into a separate field or multiply).
5. Keep a top-k heap; return top 10 with url, title, and a snippet.

**Two-stage (stretch):** BM25 as cheap recall → top ~1000 → optional re-rank
(embeddings / cross-encoder). Only after the baseline works end to end.

**PageRank (stretch):** once you have the link graph from the crawl, compute
PageRank offline and fold it into the score as a query-independent prior.

---

## 4. Search API

`axum` service:
- `GET /search?q=...&page=0` → `{ results: [{url, title, snippet, score}], took_ms }`
- Loads the term dictionary + doc-meta into RAM at startup; mmaps or seeks the
  postings file on disk.
- Target: < 10 ms for a typical query on 100k docs.

---

## Suggested crate layout

```
search-engine/
  Cargo.toml                 # workspace
  crates/
    common/   # doc types, url normalization, text pipeline (shared by all)
    crawler/  # frontier, host workers, robots, dedup, doc store
    indexer/  # tokenize→normalize→stem→stopword→postings→on-disk index
    ranker/   # index reader + BM25 scoring
    api/      # axum server wiring ranker to HTTP
```
`common` holds the text pipeline so the crawler, indexer, and query path all
normalize identically — this consistency is what makes search actually work.

---

## Build order (each step is runnable)

1. **`common`**: URL normalization + the text pipeline (tokenize→…→stopwords),
   with unit tests. Everything depends on this.
2. **Crawler, tiny**: single host, no concurrency, 100 pages of Wikipedia to a
   doc store. Prove fetch→clean→store→dedup works.
3. **Crawler, real**: host-sharded async workers, robots.txt, rate limit,
   checkpointing. Crawl to the page cap.
4. **Indexer**: build the inverted index + doc-meta on disk from the doc store.
   Verify a term's postings by hand.
5. **Ranker**: index reader + BM25. A CLI `search "query"` that prints top 10.
6. **API**: wrap the ranker in axum.
7. **Stretch**: PageRank prior → snippet highlighting → ML re-rank.

Ship steps 1–6 first. That is a complete, fast, honest search engine.
