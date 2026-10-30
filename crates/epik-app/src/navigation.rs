//! Where the app's windows may go.

/// Keeps a window on Epik's own pages: any other web page opens in the
/// browser instead, however the link was followed.
pub fn stay_in_app(url: &tauri::Url) -> bool {
    let ours = is_ours(url);
    if !ours && matches!(url.scheme(), "http" | "https") {
        let _ = open::that_detached(url.as_str());
    }
    ours
}

/// Whether `url` is one of the app's own pages: scheme `tauri`, or host
/// `tauri.localhost`, which is how Windows serves them.
fn is_ours(url: &tauri::Url) -> bool {
    url.scheme() == "tauri" || url.host_str() == Some("tauri.localhost")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ours(url: &str) -> bool {
        is_ours(&url.parse().expect("a valid URL"))
    }

    #[test]
    fn the_apps_own_pages_are_ours() {
        assert!(ours("tauri://localhost/index.html#/"));
        assert!(ours("https://tauri.localhost/index.html"));
    }

    #[test]
    fn other_web_pages_are_not() {
        assert!(!ours("https://github.com/epik-agent/Epik/issues/1"));
        assert!(!ours("http://example.com"));
    }
}
