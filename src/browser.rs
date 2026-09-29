use std::process::{Command, Stdio};

use crate::{fsx, paths};

/// A page that sends the browser on to `url`. The browser is handed this
/// file rather than the URL, because a command line is readable by every
/// local user and these URLs carry the page's token or a sign-in's state.
fn redirect_page(url: &str) -> String {
    let attr = url
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;");
    let js = serde_json::to_string(url)
        .unwrap_or_default()
        .replace('<', "\\u003c");
    format!(
        "<!doctype html><meta charset=utf-8><meta name=referrer content=no-referrer>\
         <meta http-equiv=refresh content=\"0;url={attr}\">\
         <script>location.replace({js})</script><a href=\"{attr}\">Continue</a>\n"
    )
}

pub fn open(url: &str) {
    let page = paths::runtime_dir().join("open.html");
    if fsx::write_private(&page, &redirect_page(url)).is_err() {
        return;
    }
    let _ = Command::new("xdg-open")
        .arg(&page)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redirect_page_quotes_the_url_for_both_of_its_uses() {
        let page = redirect_page("http://127.0.0.1:1/#a&b\"<c");
        assert!(page.contains("url=http://127.0.0.1:1/#a&amp;b&quot;&lt;c\""));
        assert!(page.contains(r#"location.replace("http://127.0.0.1:1/#a&b\"\u003cc")"#));
        assert!(!page.contains("\"<c"));
    }
}
