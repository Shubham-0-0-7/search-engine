use scraper::{ElementRef, Html, Selector};

pub struct Page {
    pub title: String,
    pub text: String,
    pub links: Vec<String>,
}

const SKIP_TAGS: &[&str] = &[
    "script", "style", "noscript", "table", "sup", "figure", "nav", "form", "button", "svg",
];

fn collect_text(el: ElementRef, out: &mut String) {
    for child in el.children() {
        if let Some(e) = ElementRef::wrap(child) {
            let name = e.value().name();
            let class = e.value().attr("class").unwrap_or("");
            if SKIP_TAGS.contains(&name)
                || class.contains("mw-editsection")
                || class.contains("navbox")
                || class.contains("reference")
                || class.contains("hatnote")
                || class.contains("thumb")
            {
                continue;
            }
            collect_text(e, out);
            if matches!(name, "p" | "li" | "h1" | "h2" | "h3" | "h4" | "div" | "br") {
                out.push('\n');
            }
        } else if let Some(t) = child.value().as_text() {
            out.push_str(t);
        }
    }
}

/// Extract the title, main-content text and outlinks. Prefers Wikipedia's
/// `#mw-content-text` container, falling back to `<body>`.
pub fn extract(html: &str, base: &url::Url) -> Page {
    let doc = Html::parse_document(html);
    let sel = |s| Selector::parse(s).unwrap();

    let title = ["h1#firstHeading", "h1", "title"]
        .iter()
        .find_map(|s| doc.select(&sel(s)).next())
        .map(|e| e.text().collect::<String>().trim().to_string())
        .unwrap_or_default();

    let root = doc
        .select(&sel("#mw-content-text"))
        .next()
        .or_else(|| doc.select(&sel("body")).next());

    let mut text = String::new();
    let mut links = Vec::new();
    if let Some(root) = root {
        collect_text(root, &mut text);
        for a in root.select(&sel("a[href]")) {
            if let Some(h) = a.value().attr("href") {
                if let Ok(u) = base.join(h) {
                    links.push(u.to_string());
                }
            }
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Page { title, text, links }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_wikipedia_shape() {
        let html = r#"<html><body><h1 id="firstHeading">Rust</h1>
        <div id="mw-content-text"><p>Rust is <a href="/wiki/Language">a language</a>.</p>
        <table><tr><td>junk</td></tr></table><script>x()</script></div></body></html>"#;
        let base = url::Url::parse("https://en.wikipedia.org/wiki/Rust").unwrap();
        let p = extract(html, &base);
        assert_eq!(p.title, "Rust");
        assert_eq!(p.text, "Rust is a language.");
        assert_eq!(p.links, vec!["https://en.wikipedia.org/wiki/Language"]);
    }
}
