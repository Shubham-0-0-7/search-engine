use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

/// URL frontier sharded by host. Each host has its own queue and a
/// "next allowed fetch" time, which is what enforces politeness.
pub struct Frontier {
    queues: HashMap<String, VecDeque<String>>,
    next_ok: HashMap<String, Instant>,
    in_flight: HashSet<String>,
    seen: HashSet<String>,
    delay: Duration,
}

impl Frontier {
    pub fn new(delay: Duration) -> Self {
        Self {
            queues: HashMap::new(),
            next_ok: HashMap::new(),
            in_flight: HashSet::new(),
            seen: HashSet::new(),
            delay,
        }
    }

    /// Returns true if the (already normalized) URL was new and enqueued.
    pub fn push(&mut self, url: String) -> bool {
        if !self.seen.insert(url.clone()) {
            return false;
        }
        if let Some(host) = common::url_norm::host(&url) {
            self.queues.entry(host).or_default().push_back(url);
            true
        } else {
            false
        }
    }

    /// Pop a URL from some host that is idle and past its delay.
    pub fn pop_ready(&mut self) -> Option<(String, String)> {
        let now = Instant::now();
        for (host, q) in self.queues.iter_mut() {
            if q.is_empty() || self.in_flight.contains(host) {
                continue;
            }
            if self.next_ok.get(host).is_some_and(|t| *t > now) {
                continue;
            }
            let url = q.pop_front().unwrap();
            self.in_flight.insert(host.clone());
            return Some((host.clone(), url));
        }
        None
    }

    pub fn done(&mut self, host: &str, extra_delay: Option<Duration>) {
        self.in_flight.remove(host);
        let d = extra_delay.unwrap_or(self.delay).max(self.delay);
        self.next_ok.insert(host.to_string(), Instant::now() + d);
    }

    pub fn pending(&self) -> usize {
        self.queues.values().map(|q| q.len()).sum()
    }
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }
    pub fn seen_count(&self) -> usize {
        self.seen.len()
    }
}
