//! A real HTTP client for GPUI's image loader.
//!
//! GPUI's default client refuses every request, so a remote image would show
//! its placeholder instead of loading. This module supplies a `reqwest` client.
//! The request runs on its own thread because `reqwest::blocking` creates a
//! runtime and must not block the UI executor.

use anyhow::{Context as _, Result};
use futures::{AsyncReadExt, FutureExt, channel::oneshot};
use gpui::http_client::{AsyncBody, HttpClient, Url, http};

pub(crate) struct ReqwestClient {
    client: reqwest::blocking::Client,
    user_agent: http::HeaderValue,
}

impl ReqwestClient {
    pub(crate) fn new() -> Result<Self> {
        let user_agent = http::HeaderValue::from_static("Bromine");
        let client = reqwest::blocking::Client::builder()
            .user_agent("Bromine")
            .build()
            .context("building the HTTP client")?;
        Ok(Self { client, user_agent })
    }
}

impl HttpClient for ReqwestClient {
    fn type_name(&self) -> &'static str {
        "ReqwestClient"
    }

    fn user_agent(&self) -> Option<&http::HeaderValue> {
        Some(&self.user_agent)
    }

    fn send(
        &self,
        request: http::Request<AsyncBody>,
    ) -> futures::future::BoxFuture<'static, Result<http::Response<AsyncBody>>> {
        let client = self.client.clone();
        async move {
            let (parts, mut body) = request.into_parts();
            let mut body_bytes = Vec::new();
            body.read_to_end(&mut body_bytes)
                .await
                .context("reading the request body")?;

            let (sender, receiver) = oneshot::channel();
            std::thread::spawn(move || {
                let _ = sender.send(send_blocking(client, parts, body_bytes));
            });
            receiver.await.context("the HTTP request thread stopped")?
        }
        .boxed()
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }
}

fn send_blocking(
    client: reqwest::blocking::Client,
    parts: http::request::Parts,
    body: Vec<u8>,
) -> Result<http::Response<AsyncBody>> {
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())?;
    let url = reqwest::Url::parse(&parts.uri.to_string())?;
    let mut request = client.request(method, url);
    for (name, value) in parts.headers.iter() {
        request = request.header(name.as_str(), value.as_bytes());
    }

    let response = request
        .body(body)
        .send()
        .context("sending the HTTP request")?;
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.bytes().context("reading the HTTP response")?;

    let mut builder = http::Response::builder().status(status.as_u16());
    for (name, value) in headers.iter() {
        builder = builder.header(name.as_str(), value.as_bytes());
    }
    Ok(builder.body(AsyncBody::from_bytes(bytes))?)
}
