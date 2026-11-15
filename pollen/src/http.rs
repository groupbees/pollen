//! HTTP downloads for `well-known` sources, with each origin's token sent to
//! that origin only.

use anyhow::{Context, Result, bail};
use reqwest::{StatusCode, Url, header};

use crate::credentials::Credentials;

/// How many redirects one download follows before giving up.
const MAX_REDIRECTS: usize = 10;

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
    credentials: Credentials,
}

impl Client {
    /// Build a client that sends the tokens `credentials` provide.
    ///
    /// # Errors
    ///
    /// When the TLS backend cannot be set up.
    pub fn new(credentials: Credentials) -> Result<Self> {
        // reqwest is built without a default rustls provider. An error means
        // one is installed already, by an earlier client or the host process.
        #[cfg(not(windows))]
        let _ = rustls::crypto::ring::default_provider().install_default();
        let inner = reqwest::Client::builder()
            .user_agent(concat!("pollen/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("cannot set up the HTTP client")?;
        Ok(Self { inner, credentials })
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
            let token = self.credentials.token(&origin).await?;
            let mut request = self.inner.get(url.clone());
            if let Some(token) = &token {
                request = request.bearer_auth(token.expose());
            }
            tracing::debug!(%url, authenticated = token.is_some(), "GET");
            let mut response = request
                .send()
                .await
                .with_context(|| format!("cannot reach {url}"))?;
            let status = response.status();

            if status.is_redirection() {
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .with_context(|| format!("{url} answered {status} without a location"))?;
                url = url
                    .join(location)
                    .with_context(|| format!("{url} redirects to an invalid URL"))?;
                continue;
            }
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                if token.is_some() {
                    bail!("{origin} refused the token pollen sent ({status}) for {url}");
                }
                bail!(
                    "{origin} requires authentication ({status}) for {url}; declare a token for it in the credentials file"
                );
            }
            if !status.is_success() {
                bail!("{url} answered {status}");
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
                bail!("{url} is larger than the {limit}-byte limit");
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .with_context(|| format!("cannot read {url}"))?
            {
                bytes.extend_from_slice(&chunk);
                if bytes.len() as u64 > limit {
                    bail!("{url} is larger than the {limit}-byte limit");
                }
            }
            return Ok(Download {
                bytes,
                content_type,
                url,
            });
        }
        bail!("{url} redirects more than {MAX_REDIRECTS} times")
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
