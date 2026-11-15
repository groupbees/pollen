//! HTTP downloads for `well-known` sources, with each origin's token sent to
//! that origin only.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{StatusCode, Url, header};

use crate::credentials::Credentials;

/// How many redirects one download follows before giving up.
const MAX_REDIRECTS: usize = 10;

/// How long a connection may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a download may stay silent, waiting for the response or for the
/// next chunk of its body. There is no total timeout: it would cut a large
/// artifact on a slow link that is still making progress.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// `url` as it appears in messages: no query string, fragment or userinfo,
/// since a presigned URL carries its signature there.
fn display(url: &Url) -> String {
    let mut url = url.clone();
    url.set_query(None);
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.to_string()
}

/// Explain a failed request or body read, naming the URL without its query.
fn failure(error: reqwest::Error, url: &Url, action: &str, timeouts: Timeouts) -> anyhow::Error {
    let shown = display(url);
    let error = error.without_url();
    if error.is_timeout() {
        return if error.is_connect() {
            anyhow::anyhow!(
                "{shown} did not accept a connection within {} s",
                timeouts.connect.as_secs()
            )
        } else {
            anyhow::anyhow!(
                "{shown} went silent for {} s while pollen tried to {action} it",
                timeouts.read.as_secs()
            )
        };
    }
    anyhow::Error::new(error).context(format!("cannot {action} {shown}"))
}

#[derive(Debug, Clone, Copy)]
struct Timeouts {
    connect: Duration,
    read: Duration,
}

/// A response body read whole, and what the server said it was.
#[derive(Debug)]
pub struct Download {
    /// The body.
    pub bytes: Vec<u8>,
    /// The `Content-Type` header, without its parameters.
    pub content_type: Option<String>,
    /// The URL the body came from, after redirects.
    pub url: Url,
}

/// An HTTP client that authenticates to the origins the credentials file
/// names.
///
/// Redirects are followed here rather than by `reqwest`, so the decision to
/// attach a token is taken again, against the exact origin, on every hop.
#[derive(Debug)]
pub struct Client {
    inner: reqwest::Client,
    timeouts: Timeouts,
    credentials: Credentials,
}

impl Client {
    /// Build a client that sends the tokens `credentials` provide.
    ///
    /// # Errors
    ///
    /// When the TLS backend cannot be set up.
    pub fn new(credentials: Credentials) -> Result<Self> {
        Self::with_timeouts(
            credentials,
            Timeouts {
                connect: CONNECT_TIMEOUT,
                read: READ_TIMEOUT,
            },
        )
    }

    fn with_timeouts(credentials: Credentials, timeouts: Timeouts) -> Result<Self> {
        // reqwest is built without a default rustls provider. An error means
        // one is installed already, by an earlier client or the host process.
        #[cfg(not(windows))]
        let _ = rustls::crypto::ring::default_provider().install_default();
        let inner = reqwest::Client::builder()
            .user_agent(concat!("pollen/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(timeouts.connect)
            .read_timeout(timeouts.read)
            .build()
            .context("cannot set up the HTTP client")?;
        Ok(Self {
            inner,
            timeouts,
            credentials,
        })
    }

    /// Download `url`, refusing a body larger than `limit` bytes.
    ///
    /// # Errors
    ///
    /// When the request fails, the server answers with an error status or
    /// too many redirects, refuses the credentials, or the body exceeds
    /// `limit`.
    pub async fn get(&self, url: &Url, limit: u64) -> Result<Download> {
        let mut url = url.clone();
        for _ in 0..=MAX_REDIRECTS {
            let origin = url.origin().ascii_serialization();
            let shown = display(&url);
            let token = self.credentials.token(&origin).await?;
            let mut request = self.inner.get(url.clone());
            if let Some(token) = &token {
                request = request.bearer_auth(token.expose());
            }
            tracing::debug!(url = %shown, authenticated = token.is_some(), "GET");
            let mut response = request
                .send()
                .await
                .map_err(|error| failure(error, &url, "reach", self.timeouts))?;
            let status = response.status();

            if status.is_redirection() {
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .with_context(|| format!("{shown} answered {status} without a location"))?;
                url = url
                    .join(location)
                    .with_context(|| format!("{shown} redirects to an invalid URL"))?;
                continue;
            }
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                if token.is_some() {
                    bail!("{origin} refused the token pollen sent ({status}) for {shown}");
                }
                bail!(
                    "{origin} requires authentication ({status}) for {shown}; declare a token for it in the credentials file"
                );
            }
            if !status.is_success() {
                bail!("{shown} answered {status}");
            }

            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .map(|value| value.trim().to_ascii_lowercase());
            if response
                .content_length()
                .is_some_and(|length| length > limit)
            {
                bail!("{shown} is larger than the {limit}-byte limit");
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| failure(error, &url, "read", self.timeouts))?
            {
                bytes.extend_from_slice(&chunk);
                if bytes.len() as u64 > limit {
                    bail!("{shown} is larger than the {limit}-byte limit");
                }
            }
            return Ok(Download {
                bytes,
                content_type,
                url,
            });
        }
        bail!(
            "{} redirects more than {MAX_REDIRECTS} times",
            display(&url)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn credentials_for(origin: &str, variable: &str) -> Credentials {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.yaml");
        std::fs::write(
            &path,
            format!("credentials:\n  - origin: {origin}\n    env: {variable}\n"),
        )
        .unwrap();
        Credentials::load(&path, true).unwrap()
    }

    fn url(server: &MockServer, path: &str) -> Url {
        Url::parse(&format!("{}{path}", server.uri())).unwrap()
    }

    #[tokio::test]
    async fn sends_the_token_to_its_origin_only_across_redirects() {
        let home = MockServer::start().await;
        let elsewhere = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.json"))
            .and(header("authorization", "Bearer s3cret"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/artifact", elsewhere.uri())),
            )
            .expect(1)
            .mount(&home)
            .await;
        Mock::given(method("GET"))
            .and(path("/artifact"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&elsewhere)
            .await;
        Mock::given(method("GET"))
            .and(path("/artifact"))
            .respond_with(ResponseTemplate::new(200).set_body_string("body"))
            .expect(1)
            .mount(&elsewhere)
            .await;
        let client = Client::new(helper_credentials(&home.uri(), "s3cret")).unwrap();

        let download = client.get(&url(&home, "/index.json"), 1024).await.unwrap();

        assert_eq!(download.bytes, b"body");
        assert_eq!(download.url, url(&elsewhere, "/artifact"));
    }

    #[tokio::test]
    async fn names_the_origin_when_it_refuses_the_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = Client::new(Credentials::default()).unwrap();

        let error = client
            .get(&url(&server, "/x"), 1024)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains(&server.uri()), "{error}");
        assert!(error.contains("requires authentication"), "{error}");
    }

    #[tokio::test]
    async fn refuses_a_body_over_the_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; 2048]))
            .mount(&server)
            .await;
        let client = Client::new(Credentials::default()).unwrap();

        assert!(client.get(&url(&server, "/x"), 1024).await.is_err());
    }

    #[tokio::test]
    async fn an_unset_variable_fails_before_any_request() {
        let server = MockServer::start().await;
        let client = Client::new(credentials_for(
            &server.uri(),
            "POLLEN_TEST_UNSET_HTTP_TOKEN",
        ))
        .unwrap();

        assert!(client.get(&url(&server, "/x"), 1024).await.is_err());
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    fn quick_client() -> Client {
        Client::with_timeouts(
            Credentials::default(),
            Timeouts {
                connect: Duration::from_millis(500),
                read: Duration::from_millis(300),
            },
        )
        .unwrap()
    }

    /// A server that accepts connections and never answers.
    async fn silent_server() -> (tokio::net::TcpListener, Url) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!(
            "http://{}/artifact?X-Signature=s3cret",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        (listener, url)
    }

    #[tokio::test]
    async fn gives_up_on_a_server_that_never_answers() {
        let (listener, url) = silent_server().await;
        let held = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        let error = format!("{:#}", quick_client().get(&url, 1024).await.unwrap_err());
        held.abort();

        assert!(error.contains("went silent"), "{error}");
        assert!(error.contains("/artifact"), "{error}");
        assert!(!error.contains("s3cret"), "{error}");
    }

    #[tokio::test]
    async fn gives_up_on_a_body_that_stalls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (listener, url) = silent_server().await;
        let held = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            // Answering before the request is read makes hyper drop the connection.
            let _ = stream.read(&mut [0; 1024]).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\nabc")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });

        let error = format!("{:#}", quick_client().get(&url, 1024).await.unwrap_err());
        held.abort();

        assert!(error.contains("went silent"), "{error}");
        assert!(!error.contains("s3cret"), "{error}");
    }

    #[test]
    fn display_drops_the_query_and_userinfo() {
        let url = Url::parse("https://user:pw@host.example/a?sig=s3cret#frag").unwrap();

        assert_eq!(display(&url), "https://host.example/a");
    }

    /// Credentials whose helper prints `token`. `set_var` is unsafe in
    /// edition 2024, which the crate forbids, so the token comes from a
    /// command: `git`, on `PATH` wherever the tests run, on every platform.
    fn helper_credentials(origin: &str, token: &str) -> Credentials {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.yaml");
        std::fs::write(
            &path,
            format!(
                "credentials:\n  - origin: {origin}\n    command: [git, -c, 'alias.token=!echo {token}', token]\n"
            ),
        )
        .unwrap();
        Credentials::load(&path, true).unwrap()
    }
}
