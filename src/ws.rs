//! Mattermost websocket. One blocking read on this thread; the UI stays on
//! `ControlFlow::Wait` until a note is queued. Reconnect sleeps. No poll.

use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tungstenite::Message;
use tungstenite::client::client_with_config;
use tungstenite::protocol::WebSocketConfig;

use crate::core::Link;
use crate::mattermost::{self, Incoming, ParsedPost};

pub enum LiveNote {
    State(Link),
    Posted(ParsedPost),
    Edited(ParsedPost),
    Deleted { id: String, channel_id: String },
    AuthExpired,
}

enum Stop {
    Shutdown,
    Auth,
}

pub struct Socket {
    stop: Arc<AtomicBool>,
    killer: Arc<Mutex<Option<TcpStream>>>,
    thread: Option<JoinHandle<()>>,
}

impl Socket {
    pub fn start(url: String, token: String, note: impl Fn(LiveNote) + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let killer = Arc::new(Mutex::new(None));
        let stop_thread = Arc::clone(&stop);
        let killer_thread = Arc::clone(&killer);
        let thread = thread::Builder::new()
            .name("comport-ws".into())
            .spawn(move || run(url, token, note, stop_thread, killer_thread))
            .expect("spawn websocket");
        Self {
            stop,
            killer,
            thread: Some(thread),
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(stream) = self.killer.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    url: String,
    token: String,
    note: impl Fn(LiveNote),
    stop: Arc<AtomicBool>,
    killer: Arc<Mutex<Option<TcpStream>>>,
) {
    let mut attempt = 0u32;
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let state = if attempt == 0 {
            Link::Connecting
        } else {
            Link::Reconnecting
        };
        note(LiveNote::State(state));
        match connect_and_serve(&url, &token, &note, &stop, &killer) {
            Ok(Stop::Auth) => {
                note(LiveNote::AuthExpired);
                return;
            }
            Ok(Stop::Shutdown) => return,
            Err(error) => {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                log::debug!("mattermost websocket closed: {error:#}");
                attempt = attempt.saturating_add(1);
                let secs = backoff(attempt);
                if !sleep_interruptible(&stop, Duration::from_secs(secs)) {
                    return;
                }
            }
        }
    }
}

fn connect_and_serve(
    url: &str,
    token: &str,
    note: &impl Fn(LiveNote),
    stop: &AtomicBool,
    killer: &Mutex<Option<TcpStream>>,
) -> Result<Stop> {
    let _clear = Clear(killer);
    let (host, port) = split_ws(url)?;
    let (mut socket, local) = tls_socket(&host, port, url)?;
    if let Ok(mut slot) = killer.lock() {
        *slot = local.try_clone().ok();
    }
    let auth = serde_json::json!({
        "seq": 1,
        "action": "authentication_challenge",
        "data": { "token": token }
    });
    socket
        .send(Message::Text(auth.to_string().into()))
        .map_err(|error| anyhow!("websocket auth: {error}"))?;
    // Bound the handshake, then block with no timeout so an idle socket
    // does not wake this thread or the UI.
    local.set_read_timeout(Some(Duration::from_secs(20)))?;
    let mut authed = false;
    let mut ignored = 0u8;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(Stop::Shutdown);
        }
        let message = socket.read().map_err(|error| anyhow!("{error}"))?;
        if matches!(message, Message::Ping(_)) {
            let _ = socket.flush();
        }
        if let Some(incoming) = incoming_from(message)? {
            match incoming {
                Incoming::AuthOk => {
                    authed = true;
                    local.set_read_timeout(None)?;
                    note(LiveNote::State(Link::Live));
                }
                Incoming::AuthFail => return Ok(Stop::Auth),
                Incoming::Posted(post) => note(LiveNote::Posted(post)),
                Incoming::Edited(post) => note(LiveNote::Edited(post)),
                Incoming::Deleted { id, channel_id } => {
                    note(LiveNote::Deleted { id, channel_id });
                }
            }
        } else if !authed {
            ignored += 1;
            if ignored > 8 {
                anyhow::bail!("websocket did not authenticate");
            }
        }
    }
}

fn incoming_from(message: Message) -> Result<Option<Incoming>> {
    match message {
        Message::Text(text) => Ok(mattermost::parse_ws_message(text.as_ref())),
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Ok(None),
        Message::Binary(_) => Ok(None),
        Message::Close(_) => Err(anyhow!("websocket closed")),
    }
}

fn tls_socket(
    host: &str,
    port: u16,
    url: &str,
) -> Result<(
    tungstenite::WebSocket<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>,
    TcpStream,
)> {
    let roots = rustls_native_certs::load_native_certs();
    if roots.certs.is_empty() {
        anyhow::bail!("no usable TLS trust roots");
    }
    let mut store = rustls::RootCertStore::empty();
    for cert in roots.certs {
        let _ = store.add(cert);
    }
    let mut config =
        rustls::ClientConfig::builder_with_provider(Arc::new(crate::provider::provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|error| anyhow!("tls config: {error}"))?
            .with_root_certificates(store)
            .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let name = rustls::pki_types::ServerName::try_from(host)
        .map_err(|_| anyhow!("invalid websocket host"))?
        .to_owned();
    let conn = rustls::ClientConnection::new(Arc::new(config), name)
        .map_err(|error| anyhow!("tls: {error}"))?;
    let addr = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let tcp = TcpStream::connect(&addr).with_context(|| format!("connect {addr}"))?;
    tcp.set_nodelay(true)?;
    let local = tcp.try_clone()?;
    let stream = rustls::StreamOwned::new(conn, tcp);
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(8 * 1024 * 1024);
    config.max_frame_size = Some(8 * 1024 * 1024);
    let (socket, _response) = client_with_config(url, stream, Some(config))
        .map_err(|error| anyhow!("websocket handshake: {error}"))?;
    Ok((socket, local))
}

fn split_ws(url: &str) -> Result<(String, u16)> {
    let rest = url
        .strip_prefix("wss://")
        .context("websocket URL must be wss")?;
    let hostport = rest.split('/').next().unwrap_or(rest);
    anyhow::ensure!(!hostport.is_empty(), "websocket URL has no host");
    if let Some(host) = hostport.strip_prefix('[') {
        let (host, rest) = host.split_once(']').context("bad ipv6 host")?;
        let port = if let Some(port) = rest.strip_prefix(':') {
            port.parse().context("bad websocket port")?
        } else {
            443
        };
        return Ok((host.to_string(), port));
    }
    if let Some((host, port)) = hostport.rsplit_once(':')
        && !host.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Ok((host.to_string(), port.parse()?));
    }
    Ok((hostport.to_string(), 443))
}

fn backoff(attempt: u32) -> u64 {
    (1u64 << (attempt.saturating_sub(1)).min(6)).min(60)
}

fn sleep_interruptible(stop: &AtomicBool, total: Duration) -> bool {
    let step = Duration::from_millis(200);
    let mut left = total;
    while !left.is_zero() {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let slice = step.min(left);
        thread::sleep(slice);
        left = left.saturating_sub(slice);
    }
    !stop.load(Ordering::Relaxed)
}

struct Clear<'a>(&'a Mutex<Option<TcpStream>>);

impl Drop for Clear<'_> {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = None;
        }
    }
}
