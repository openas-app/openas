//! Pulling the app out of a model's reply, and the static checks that need no
//! browser.

use regex::Regex;

pub struct Parts {
    pub html: Option<String>,
    pub svg: Option<String>,
}

/// The html block runs to the LAST fence before the svg block (or the end):
/// some models put fences inside their code or nest a second ```html.
pub fn parts(reply: &str) -> Parts {
    let svg_at = reply.rfind("```svg");
    let html = reply.find("```html").map(|start| {
        let body_start = start + "```html".len();
        let end_limit = svg_at.filter(|&s| s > body_start).unwrap_or(reply.len());
        let region = &reply[body_start..end_limit];
        let region = region.rfind("```").map(|e| &region[..e]).unwrap_or(region);
        // A nested "```html" line at the top: drop it.
        let mut s = region.trim().to_string();
        while let Some(rest) = s.strip_prefix("```html") {
            s = rest.trim().to_string();
        }
        s
    });
    let html = html.or_else(|| {
        let re = Regex::new(r"(?is)(<!doctype html.*</html>)").unwrap();
        re.captures(reply).map(|c| c[1].to_string())
    });
    let svg = svg_at.and_then(|s| {
        let rest = &reply[s + "```svg".len()..];
        let rest = rest.find("```").map(|e| &rest[..e]).unwrap_or(rest);
        let t = rest.trim();
        t.contains("<svg").then(|| t.to_string())
    });
    Parts { html, svg }
}

/// The page as written so far, while the reply is still coming: from the
/// opening ```html fence to the end, or to where the icon's block starts.
pub fn partial_html(text: &str) -> Option<&str> {
    let start = text.find("```html")?;
    let body = &text[start + 7..];
    let body = body
        .strip_prefix('\n')
        .or_else(|| body.find('\n').map(|i| &body[i + 1..]))?;
    let end = body.find("\n```").unwrap_or(body.len());
    Some(&body[..end])
}

pub fn title(html: &str) -> Option<String> {
    let re = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap();
    re.captures(html)
        .map(|c| c[1].trim().to_string())
        .filter(|t| !t.is_empty() && t.len() <= 40)
}

/// Problems visible in the source alone.
pub fn static_problems(html: &str) -> Vec<String> {
    let mut out = vec![];
    if !html.to_ascii_lowercase().contains("</html>") {
        out.push("The file is unfinished: it has no closing </html>.".into());
    }
    let dialogs = Regex::new(r"(^|[^\w.$])(window\.)?(alert|confirm|prompt)\s*\(").unwrap();
    if let Some(c) = dialogs.captures(html) {
        out.push(format!(
            "It calls {}(); use in-page UI instead (these do nothing in the app window).",
            &c[3]
        ));
    }
    if html.contains("localStorage") || html.contains("indexedDB") {
        out.push("It uses localStorage or indexedDB; save with app.store.get/set instead.".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_html_to_the_last_fence_and_the_icon() {
        let r = "```html\n```html\n<!doctype html><title>Habits</title><script>let s=`a`;</script></html>\n```\n```svg\n<svg viewBox=\"0 0 1024 1024\"></svg>\n```";
        let p = parts(r);
        assert!(p.html.as_deref().unwrap().starts_with("<!doctype html>"));
        assert!(p.html.as_deref().unwrap().ends_with("</html>"));
        assert!(p.svg.unwrap().starts_with("<svg"));
        assert_eq!(title(&parts(r).html.unwrap()).as_deref(), Some("Habits"));
    }

    #[test]
    fn finds_dialogs_but_not_app_ask_or_method_names() {
        assert!(!static_problems(
            "<html>app.ask(prompt); x.confirm(); const confirmed = 1;</html>"
        )
        .iter()
        .any(|p| p.contains("calls")));
        assert!(static_problems("<html>if (confirm('x')) {}</html>")
            .iter()
            .any(|p| p.contains("confirm")));
        assert!(static_problems("<html>")
            .iter()
            .any(|p| p.contains("unfinished")));
    }
}
