use url::Url;

/// Canonicalize a URL for the URL-seen set: http(s) only, lowercase host,
/// no fragment, sorted query params, no default port, no trailing slash
/// (except root).
pub fn normalize(raw: &str) -> Option<String> {
    let mut u = Url::parse(raw).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    u.host_str()?;
    u.set_fragment(None);

    if u.query().is_some() {
        let mut pairs: Vec<(String, String)> = u
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        pairs.sort();
        if pairs.is_empty() {
            u.set_query(None);
        } else {
            u.query_pairs_mut().clear().extend_pairs(pairs);
        }
    }

    let mut s = u.to_string();
    if u.path().len() > 1 && s.ends_with('/') && u.query().is_none() {
        s.pop();
    }
    Some(s)
}

/// Host used for politeness sharding.
pub fn host(raw: &str) -> Option<String> {
    Url::parse(raw).ok()?.host_str().map(|h| h.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_fragment_and_lowercases_host() {
        assert_eq!(
            normalize("HTTPS://En.Wikipedia.org/wiki/Rust#History").unwrap(),
            "https://en.wikipedia.org/wiki/Rust"
        );
    }

    #[test]
    fn sorts_query_and_drops_default_port() {
        assert_eq!(
            normalize("https://a.com:443/x?b=2&a=1").unwrap(),
            "https://a.com/x?a=1&b=2"
        );
    }

    #[test]
    fn trailing_slash_trimmed_except_root() {
        assert_eq!(normalize("https://a.com/x/").unwrap(), "https://a.com/x");
        assert_eq!(normalize("https://a.com").unwrap(), "https://a.com/");
    }

    #[test]
    fn rejects_non_http() {
        assert!(normalize("mailto:a@b.com").is_none());
        assert!(normalize("javascript:void(0)").is_none());
    }
}
