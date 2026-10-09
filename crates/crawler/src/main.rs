mod extract;
mod frontier;

use anyhow::Result;
use common::{url_norm, Doc};
use frontier::Frontier;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Duration;
use texting_robots::Robot;
use tokio::sync::mpsc;

const UA: &str = "ShubhamSearchBot/0.1 (learning project; kingsinghcp@gmail.com)";

struct Config {
    seeds: Vec<String>,
    max_pages: usize,
    delay: Duration,
    out: String,
    /// Only follow links whose path starts with this prefix (per allowed host).
    path_prefix: String,
}

fn parse_args() -> Config {
    let mut c = Config {
        seeds: vec![],
        max_pages: 100,
        delay: Duration::from_millis(500),
        out: "data/docs.jsonl".into(),
        path_prefix: "/wiki/".into(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--max-pages" => c.max_pages = it.next().unwrap().parse().unwrap(),
            "--delay-ms" => c.delay = Duration::from_millis(it.next().unwrap().parse().unwrap()),
            "--out" => c.out = it.next().unwrap(),
            "--path-prefix" => c.path_prefix = it.next().unwrap(),
            s => c.seeds.push(s.to_string()),
        }
    }
    if c.seeds.is_empty() {
        c.seeds.push("https://en.wikipedia.org/wiki/Search_engine".into());
    }
    c
}

enum Outcome {
    Page { host: String, url: String, title: String, text: String, links: Vec<String> },
    Skipped { host: String },
}

async fn robots_for(client: &reqwest::Client, host: &str) -> Option<Robot> {
    let body = client
        .get(format!("https://{host}/robots.txt"))
        .send()
        .await
        .ok()?
        .bytes()
        .await
        .ok()?;
    Robot::new(UA, &body).ok()
}

async fn fetch(client: reqwest::Client, host: String, url: String) -> Outcome {
    let resp = match client.get(&url).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return Outcome::Skipped { host },
    };
    let is_html = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"));
    if !is_html {
        return Outcome::Skipped { host };
    }
    let Ok(body) = resp.text().await else { return Outcome::Skipped { host } };
    let base = url::Url::parse(&url).unwrap();
    let p = extract::extract(&body, &base);
    Outcome::Page { host, url, title: p.title, text: p.text, links: p.links }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = parse_args();
    std::fs::create_dir_all(std::path::Path::new(&cfg.out).parent().unwrap())?;
    let mut out = OpenOptions::new().create(true).append(true).open(&cfg.out)?;

    let client = reqwest::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(20))
        .build()?;

    let mut frontier = Frontier::new(cfg.delay);
    let mut robots: HashMap<String, Option<Robot>> = HashMap::new();
    let mut content_seen: HashSet<[u8; 32]> = HashSet::new();
    let mut allowed_hosts: HashSet<String> = HashSet::new();
    let mut crawl_delay: HashMap<String, Duration> = HashMap::new();

    for s in &cfg.seeds {
        if let Some(n) = url_norm::normalize(s) {
            if let Some(h) = url_norm::host(&n) {
                allowed_hosts.insert(h);
            }
            frontier.push(n);
        }
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<Outcome>();
    let mut stored = 0usize;
    let mut next_id: u32 = 0;

    while stored < cfg.max_pages {
        // Launch every fetch that politeness currently allows.
        while let Some((host, url)) = frontier.pop_ready() {
            if !robots.contains_key(&host) {
                let r = robots_for(&client, &host).await;
                if let Some(d) = r.as_ref().and_then(|r| r.delay) {
                    crawl_delay.insert(host.clone(), Duration::from_secs_f32(d));
                }
                robots.insert(host.clone(), r);
            }
            let allowed = robots[&host].as_ref().map_or(true, |r| r.allowed(&url));
            if !allowed {
                frontier.done(&host, None);
                continue;
            }
            let (client, tx) = (client.clone(), tx.clone());
            tokio::spawn(async move {
                let _ = tx.send(fetch(client, host, url).await);
            });
        }

        if frontier.in_flight() == 0 && frontier.pending() == 0 {
            break;
        }

        // Wait for a result, or tick so delayed hosts get re-checked.
        let outcome = tokio::select! {
            o = rx.recv() => o,
            _ = tokio::time::sleep(Duration::from_millis(50)) => None,
        };
        let Some(outcome) = outcome else { continue };

        match outcome {
            Outcome::Skipped { host } => frontier.done(&host, crawl_delay.get(&host).copied()),
            Outcome::Page { host, url, title, text, links } => {
                frontier.done(&host, crawl_delay.get(&host).copied());
                if text.len() < 200 {
                    continue;
                }
                let hash: [u8; 32] = Sha256::digest(text.as_bytes()).into();
                if !content_seen.insert(hash) {
                    continue;
                }
                let doc = Doc { id: next_id, url: url.clone(), title: title.clone(), text };
                next_id += 1;
                writeln!(out, "{}", serde_json::to_string(&doc)?)?;
                stored += 1;
                for l in links {
                    if let Some(n) = url_norm::normalize(&l) {
                        let h = url_norm::host(&n).unwrap_or_default();
                        let path = url::Url::parse(&n).map(|u| u.path().to_string()).unwrap_or_default();
                        // Stay on seed hosts, article pages only (no Special:/File:/Talk: etc).
                        if allowed_hosts.contains(&h)
                            && path.starts_with(&cfg.path_prefix)
                            && !path[cfg.path_prefix.len()..].contains(':')
                        {
                            frontier.push(n);
                        }
                    }
                }
                if stored % 10 == 0 || stored == cfg.max_pages {
                    eprintln!(
                        "stored={stored} pending={} seen={} last={:?}",
                        frontier.pending(),
                        frontier.seen_count(),
                        title
                    );
                }
            }
        }
    }
    out.flush()?;
    eprintln!("done: {stored} docs -> {}", cfg.out);
    Ok(())
}
