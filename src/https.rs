//! Blocking HTTPS using the vendored FileMan rustls-rustcrypto provider.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};

use crate::net::{Method, Request, Response, Transport};
use crate::provider;

pub struct Https {
    agent: ureq::Agent,
    response_bytes: u64,
}

impl Https {
    pub fn from_system_roots() -> Result<Self> {
        let roots = rustls_native_certs::load_native_certs();
        if !roots.errors.is_empty() || roots.certs.is_empty() {
            anyhow::bail!("no usable TLS trust roots");
        }
        let certs = roots
            .certs
            .iter()
            .map(|root| ureq::tls::Certificate::from_der(root).to_owned());
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            .http_status_as_error(false)
            .max_redirects(3)
            .max_idle_connections(0)
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(30)))
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_resolve(Some(Duration::from_secs(30)))
            .max_response_header_size(16 * 1024)
            .user_agent(concat!("comport/", env!("CARGO_PKG_VERSION")))
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .unversioned_rustls_crypto_provider(Arc::new(provider::provider()))
                    .root_certs(certs.into())
                    .build(),
            )
            .build()
            .new_agent();
        Ok(Self {
            agent,
            response_bytes: 8 * 1024 * 1024,
        })
    }
}

impl Transport for Https {
    fn live(&self) -> bool {
        true
    }

    fn send(&self, req: &Request) -> Result<Response> {
        validate_https(&req.url)?;
        if req.body.len() > 1024 * 1024 {
            anyhow::bail!("request body exceeds 1 MiB");
        }
        let mut response = match req.method {
            Method::Get => {
                let mut builder = self.agent.get(&req.url);
                for (name, value) in &req.headers {
                    builder = builder.header(name, value);
                }
                builder.call().map_err(|error| anyhow!("{error}"))?
            }
            Method::Delete => {
                let mut builder = self.agent.delete(&req.url);
                for (name, value) in &req.headers {
                    builder = builder.header(name, value);
                }
                builder.call().map_err(|error| anyhow!("{error}"))?
            }
            Method::Post | Method::Put => {
                let mut builder = if req.method == Method::Post {
                    self.agent.post(&req.url)
                } else {
                    self.agent.put(&req.url)
                };
                for (name, value) in &req.headers {
                    builder = builder.header(name, value);
                }
                builder
                    .send(&req.body[..])
                    .map_err(|error| anyhow!("{error}"))?
            }
        };
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(self.response_bytes + 1)
            .read_to_vec()
            .map_err(|error| anyhow!("{error}"))?;
        if body.len() as u64 > self.response_bytes {
            anyhow::bail!("response exceeds 8 MiB");
        }
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

fn validate_https(url: &str) -> Result<()> {
    if url.len() > 4096 || url.contains('#') || url.chars().any(char::is_control) {
        anyhow::bail!("invalid URL");
    }
    let uri: ureq::http::Uri = url.parse().map_err(|_| anyhow!("invalid URL"))?;
    if uri.scheme_str() != Some("https")
        || uri.host().is_none_or(str::is_empty)
        || uri.authority().is_none_or(|a| a.as_str().contains('@'))
    {
        anyhow::bail!("Mattermost Site URL must be https:// without credentials");
    }
    Ok(())
}
