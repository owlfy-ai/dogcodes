//! Whether Google's sign-in service answers, the way `agy` would reach it.
//!
//! A print command that finds its token due for refresh retries silently for 10s and
//! then falls back to an interactive Google sign-in. Right after a computer wakes, DNS
//! and TLS are often not back yet, and that alone opens a sign-in page for nobody.
//! Checking first means quota waits for the network instead.
use std::{collections::BTreeMap, ffi::OsString, time::Duration};

/// Where `agy` refreshes its token.
pub(crate) const SIGN_IN_ENDPOINT: &str = "https://oauth2.googleapis.com/";

/// Half of `agy`'s own silent sign-in budget: a handshake that cannot finish in this
/// time leaves it little chance of refreshing in time.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Any HTTP answer, whatever its status, means name lookup, the proxy and TLS all work.
pub(crate) async fn reachable(endpoint: &str, proxies: &BTreeMap<String, OsString>) -> bool {
    let Ok(client) = client(endpoint, proxies) else {
        return false;
    };
    client.head(endpoint).send().await.is_ok()
}

fn client(
    endpoint: &str,
    proxies: &BTreeMap<String, OsString>,
) -> reqwest::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(CHECK_TIMEOUT);
    if let Some(proxy) = proxy_for(endpoint, proxies)? {
        builder = builder.proxy(proxy);
    }
    builder.build()
}

/// `agy` is a Go program: an https request goes through `HTTPS_PROXY` only (never
/// `ALL_PROXY`), `NO_PROXY` applies, and a value without a scheme means `http://`.
fn proxy_for(
    endpoint: &str,
    proxies: &BTreeMap<String, OsString>,
) -> reqwest::Result<Option<reqwest::Proxy>> {
    let value = |name: &str| {
        proxies
            .get(name)
            .and_then(|value| value.to_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let name = if endpoint.starts_with("https:") {
        "HTTPS_PROXY"
    } else {
        "HTTP_PROXY"
    };
    let Some(url) = value(name) else {
        return Ok(None);
    };
    let url = if url.contains("://") {
        url.to_owned()
    } else {
        format!("http://{url}")
    };
    let no_proxy = value("NO_PROXY").and_then(reqwest::NoProxy::from_string);
    Ok(Some(reqwest::Proxy::all(url)?.no_proxy(no_proxy)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Answers every request with 404, like a service with nothing at `/`, until the
    /// test ends. (A refused connection takes Windows about 2s, so no idle timeout.)
    async fn server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buffer = [0; 1024];
                let _ = stream.read(&mut buffer).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                    .await;
            }
        });
        (format!("http://{address}/"), task)
    }

    async fn closed_port() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        address.to_string()
    }

    fn environment(pairs: &[(&str, &str)]) -> BTreeMap<String, OsString> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).into()))
            .collect()
    }

    #[tokio::test]
    async fn an_error_status_still_counts_as_reachable() {
        let (url, _server) = server().await;
        assert!(reachable(&url, &BTreeMap::new()).await);
    }

    #[tokio::test]
    async fn nothing_listening_is_unreachable() {
        let url = format!("http://{}/", closed_port().await);
        assert!(!reachable(&url, &BTreeMap::new()).await);
    }

    #[tokio::test]
    async fn an_unresolvable_name_is_unreachable() {
        assert!(!reachable("http://dogcodes-sign-in.invalid/", &BTreeMap::new()).await);
    }

    #[tokio::test]
    async fn the_check_goes_through_the_proxy_agy_is_given() {
        // The endpoint itself answers, but agy would only reach it through a dead proxy.
        let (url, _server) = server().await;
        let dead = closed_port().await;
        assert!(!reachable(&url, &environment(&[("HTTP_PROXY", &dead)])).await);
        // NO_PROXY exempts it, as Go does.
        assert!(
            reachable(
                &url,
                &environment(&[("HTTP_PROXY", &dead), ("NO_PROXY", "127.0.0.1")])
            )
            .await
        );
    }

    #[tokio::test]
    async fn https_ignores_all_proxy_and_http_proxy_like_go() {
        let dead = format!("http://{}", closed_port().await);
        let proxies = environment(&[("ALL_PROXY", &dead), ("HTTP_PROXY", &dead)]);
        assert!(proxy_for(SIGN_IN_ENDPOINT, &proxies).unwrap().is_none());
        let proxies = environment(&[("HTTPS_PROXY", "127.0.0.1:7890")]);
        assert!(proxy_for(SIGN_IN_ENDPOINT, &proxies).unwrap().is_some());
    }
}
