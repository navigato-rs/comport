//! Blocking HTTP seam. Production will sit on rustls; tests and demo replay fixtures.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        let needle = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find_map(|(k, v)| (k.to_ascii_lowercase() == needle).then_some(v.as_str()))
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).context("decode JSON body")
    }
}

pub trait Transport: Send + Sync {
    fn send(&self, req: &Request) -> Result<Response>;

    /// Live HTTPS, as opposed to fixture replay. The session opens a websocket
    /// only for a live transport so tests never dial out.
    fn live(&self) -> bool {
        false
    }
}

/// Replay map keyed by METHOD + path (query stripped). Records every call.
pub struct Replay {
    map: HashMap<(String, String), Response>,
    calls: Mutex<Vec<(String, String)>>,
}

impl Replay {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn insert(&mut self, method: &str, path: &str, response: Response) {
        self.map
            .insert((method.to_ascii_uppercase(), path.to_string()), response);
    }

    pub fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().expect("replay call log").clone()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().expect("replay call log").len()
    }
}

impl Default for Replay {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for Replay {
    fn send(&self, req: &Request) -> Result<Response> {
        let path = path_only(&req.url);
        let key = (req.method.as_str().to_string(), path);
        self.calls
            .lock()
            .expect("replay call log")
            .push(key.clone());
        self.map
            .get(&key)
            .cloned()
            .ok_or_else(|| anyhow!("no fixture for {} {}", key.0, key.1))
    }
}

/// Transport that fails on any send. Used to prove a cache hit did no HTTP.
pub struct Forbidden;

impl Transport for Forbidden {
    fn send(&self, req: &Request) -> Result<Response> {
        anyhow::bail!(
            "HTTP used on a path that must be cache-only: {} {}",
            req.method.as_str(),
            req.url
        )
    }
}

pub fn path_only(url: &str) -> String {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let path = without_scheme
        .find('/')
        .map(|i| &without_scheme[i..])
        .unwrap_or("/");
    path.split('?').next().unwrap_or(path).to_string()
}

pub fn json_response(status: u16, body: &str) -> Response {
    Response {
        status,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: body.as_bytes().to_vec(),
    }
}

pub fn bytes_response(status: u16, content_type: &str, body: Vec<u8>) -> Response {
    Response {
        status,
        headers: vec![("Content-Type".into(), content_type.into())],
        body,
    }
}
