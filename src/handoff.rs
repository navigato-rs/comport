//! Browser SSO handoff. Mattermost puts the session in a one-time `server_token`
//! on the final page URL (`mattermost://…` or the https address). It is not
//! available by polling the client `desktop_token`.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::settings;

pub fn desktop_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("system rng");
    hex(&bytes)
}

pub fn browser_login_url(site: &str, token: &str) -> String {
    format!("{site}/login?desktop_token={token}")
}

/// Open the system browser. No webview.
pub fn open_browser(url: &str) -> Result<()> {
    anyhow::ensure!(
        url.starts_with("https://") && !url.contains([' ', '\n', '\r', '"', '\'']),
        "refusing to open a non-https URL"
    );
    for mut command in browser_commands(url) {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match command.spawn() {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("open the system browser"),
        }
    }
    bail!("could not open a browser (tried xdg-open, gio, and sensible-browser)")
}

fn browser_commands(url: &str) -> Vec<Command> {
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        command.arg(url);
        return vec![command];
    }
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        return vec![command];
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut xdg = Command::new("xdg-open");
        xdg.arg(url);
        let mut gio = Command::new("gio");
        gio.args(["open", url]);
        let mut sensible = Command::new("sensible-browser");
        sensible.arg(url);
        vec![xdg, gio, sensible]
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Handoff {
    pub site_url: String,
    pub server_token: String,
}

/// Accept a full redirect URL or, with `fallback_site`, a bare server token.
pub fn parse_handoff(input: &str, fallback_site: &str) -> Result<Handoff> {
    let input = input.trim();
    anyhow::ensure!(!input.is_empty(), "paste the browser address");
    if !input.contains("://") {
        let token = clean_token(input)?;
        let site = crate::mattermost::normalize_site(fallback_site)?;
        return Ok(Handoff {
            site_url: site,
            server_token: token,
        });
    }
    let (scheme, rest) = input
        .split_once("://")
        .context("paste the browser address")?;
    let scheme = scheme.to_ascii_lowercase();
    anyhow::ensure!(
        matches!(
            scheme.as_str(),
            "https" | "mattermost" | "mattermost-dev" | "comport"
        ),
        "unsupported sign-in link"
    );
    let rest = rest.split('#').next().unwrap_or(rest);
    let (host_path, query) = rest.split_once('?').context("link has no server_token")?;
    let token = query_param(query, "server_token").context("link has no server_token")?;
    let token = clean_token(&percent_decode(&token))?;
    let host_path = host_path.trim_end_matches('/');
    let base = host_path
        .strip_suffix("/login/desktop")
        .unwrap_or(host_path)
        .trim_end_matches('/');
    anyhow::ensure!(!base.is_empty() && !base.contains('@'), "bad sign-in host");
    let site = crate::mattermost::normalize_site(&format!("https://{base}"))?;
    Ok(Handoff {
        site_url: site,
        server_token: token,
    })
}

fn clean_token(token: &str) -> Result<String> {
    anyhow::ensure!(
        (20..=256).contains(&token.len())
            && token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "that does not look like a Mattermost sign-in link"
    );
    Ok(token.to_string())
}

fn query_param(query: &str, name: &str) -> Option<String> {
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == name && !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

fn socket_path() -> PathBuf {
    settings::runtime_dir().join("login.sock")
}

/// Hand the URL to a ComPort that is already running. False if none is.
pub fn try_forward(url: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;
        let Ok(mut stream) = UnixStream::connect(socket_path()) else {
            return false;
        };
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        writeln!(stream, "{url}").is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = url;
        false
    }
}

/// Listen for `mattermost://` / `comport://` from a second process.
pub struct LoginSocket {
    path: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LoginSocket {
    pub fn spawn(on_url: impl Fn(String) + Send + 'static) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            use std::os::unix::net::UnixListener;
            let path = socket_path();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).context("create runtime dir")?;
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
            }
            // A live ComPort owns the socket. This window still opens; links
            // go to the instance that bound it. A crash leaves a dead file.
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                return Ok(Self {
                    path: PathBuf::new(),
                    shutdown: Arc::new(AtomicBool::new(false)),
                    thread: None,
                });
            }
            let _ = fs::remove_file(&path);
            let listener = UnixListener::bind(&path).context("listen for sign-in links")?;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
            let shutdown = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&shutdown);
            let path_thread = path.clone();
            let thread = thread::Builder::new()
                .name("comport-login".into())
                .spawn(move || {
                    while !flag.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                if flag.load(Ordering::Relaxed) {
                                    break;
                                }
                                let mut line = String::new();
                                let _ = BufReader::new(stream).read_line(&mut line);
                                let line = line.trim();
                                if !line.is_empty() {
                                    on_url(line.to_string());
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    let _ = path_thread;
                })
                .context("spawn sign-in listener")?;
            Ok(Self {
                path,
                shutdown,
                thread: Some(thread),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = on_url;
            Ok(Self {
                path: PathBuf::new(),
                shutdown: Arc::new(AtomicBool::new(false)),
                thread: None,
            })
        }
    }
}

impl Drop for LoginSocket {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        #[cfg(unix)]
        {
            use std::os::unix::net::UnixStream;
            let _ = UnixStream::connect(&self.path);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mattermost_redirect_and_a_pasted_https_url() {
        let handoff = parse_handoff(
            "mattermost://chat.company.com/login/desktop?client_token=abc123abc123abc123abc123&server_token=servertokenvalue123456",
            "",
        )
        .unwrap();
        assert_eq!(handoff.site_url, "https://chat.company.com");
        assert_eq!(handoff.server_token, "servertokenvalue123456");

        let sub = parse_handoff(
            "https://chat.company.com/mattermost/login/desktop?server_token=servertokenvalue123456&client_token=x",
            "",
        )
        .unwrap();
        assert_eq!(sub.site_url, "https://chat.company.com/mattermost");

        let bare = parse_handoff("servertokenvalue123456", "chat.company.com").unwrap();
        assert_eq!(bare.site_url, "https://chat.company.com");
        assert_eq!(bare.server_token, "servertokenvalue123456");
    }

    #[test]
    fn browser_url_keeps_the_desktop_token_on_the_login_page() {
        let url = browser_login_url("https://chat.company.com", "abcd");
        assert_eq!(url, "https://chat.company.com/login?desktop_token=abcd");
        assert_eq!(desktop_token().len(), 64);
    }
}
