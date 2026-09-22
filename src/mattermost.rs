//! Mattermost REST client over a `Transport`. Fixture replay is the v1 path.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::cache::Cache;
use crate::core::{Message, Portrait, Room, RoomKind, Sidebar, Team, User};
use crate::emoji::expand_shortcodes;
use crate::net::{Method, Request, Response, Transport};

#[derive(Clone, Debug)]
pub struct Account {
    pub site_url: String,
    pub token: String,
    pub me: User,
    pub teams: Vec<Team>,
    pub team: Team,
    pub sidebar: Sidebar,
    pub users: Vec<User>,
    pub websocket_url: String,
}

#[derive(Deserialize)]
struct ApiUser {
    id: String,
    username: String,
    #[serde(default)]
    first_name: String,
    #[serde(default)]
    last_name: String,
    #[serde(default)]
    nickname: String,
}

impl ApiUser {
    fn into_user(self) -> User {
        let display = if !self.nickname.is_empty() {
            self.nickname.clone()
        } else if !self.first_name.is_empty() {
            format!("{} {}", self.first_name, self.last_name)
                .trim()
                .to_string()
        } else {
            self.username.clone()
        };
        User {
            id: self.id,
            username: self.username,
            display_name: display,
            avatar: None,
        }
    }
}

#[derive(Deserialize)]
struct ApiTeam {
    id: String,
    name: String,
    display_name: String,
}

#[derive(Deserialize)]
struct ApiChannel {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    name: String,
    display_name: String,
}

#[derive(Deserialize)]
struct CategoryList {
    categories: Vec<ApiCategory>,
}

#[derive(Deserialize)]
struct ApiCategory {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    channel_ids: Vec<String>,
}

#[derive(Deserialize)]
struct PostList {
    order: Vec<String>,
    posts: serde_json::Map<String, serde_json::Value>,
}

/// Trim, require https, strip a trailing slash. Bare hostnames get `https://`.
pub fn normalize_site(input: &str) -> Result<String> {
    let trimmed = input.trim().trim_end_matches('/');
    anyhow::ensure!(!trimmed.is_empty(), "enter your Mattermost Site URL");
    anyhow::ensure!(
        !trimmed.contains('@') && !trimmed.contains('#'),
        "Site URL must not include credentials or a fragment"
    );
    let url = if let Some(rest) = trimmed.strip_prefix("https://") {
        anyhow::ensure!(!rest.is_empty(), "enter a hostname after https://");
        format!("https://{rest}")
    } else if trimmed.starts_with("http://") {
        anyhow::bail!("HTTPS is required (http:// is not used for login)");
    } else {
        format!("https://{trimmed}")
    };
    Ok(url)
}

fn send(
    transport: &dyn Transport,
    site: &str,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Vec<u8>,
) -> Result<Response> {
    let mut headers = vec![("Content-Type".into(), "application/json".into())];
    if let Some(token) = token {
        headers.push(("Authorization".into(), format!("Bearer {token}")));
    }
    let response = transport.send(&Request {
        method,
        url: format!("{site}{path}"),
        headers,
        body,
    })?;
    if response.status >= 400 {
        let detail = server_message(&response);
        if detail.is_empty() {
            bail!(
                "{} {path} returned HTTP {}",
                method.as_str(),
                response.status
            );
        }
        bail!(
            "{} {path} returned HTTP {}: {detail}",
            method.as_str(),
            response.status
        );
    }
    Ok(response)
}

fn server_message(response: &Response) -> String {
    let message = serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .and_then(|message| message.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let message = message.replace(['\n', '\r'], " ");
    message.chars().take(180).collect()
}

fn empty_account(site: &str, token: String, me: User) -> Account {
    Account {
        site_url: site.to_string(),
        token,
        me,
        teams: Vec::new(),
        team: Team {
            id: String::new(),
            name: String::new(),
            display_name: String::new(),
        },
        sidebar: Sidebar::default(),
        users: Vec::new(),
        websocket_url: websocket_url(site, ""),
    }
}

/// Password login. Token comes from the `Token` response header (Mattermost).
pub fn login(
    transport: &dyn Transport,
    site: &str,
    login_id: &str,
    password: &str,
    totp: Option<&str>,
) -> Result<Account> {
    let mut payload = serde_json::json!({
        "login_id": login_id,
        "password": password,
    });
    if let Some(totp) = totp.filter(|t| !t.is_empty()) {
        payload["token"] = serde_json::Value::String(totp.to_string());
    }
    let body = serde_json::to_vec(&payload)?;
    let response = transport.send(&Request {
        method: Method::Post,
        url: format!("{site}/api/v4/users/login"),
        headers: vec![("Content-Type".into(), "application/json".into())],
        body,
    })?;
    if response.status == 401 {
        let id = serde_json::from_slice::<serde_json::Value>(&response.body)
            .ok()
            .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(str::to_string))
            .unwrap_or_default();
        if id.contains("mfa") {
            anyhow::bail!("mfa_required");
        }
        if let Ok(config) = fetch_client_config(transport, site)
            && config.sso()
        {
            let via = config.providers().join(", ");
            anyhow::bail!(
                "Password sign-in was rejected. This server uses single sign-on ({via}). Use Sign in with browser — a personal access token is not required."
            );
        }
        anyhow::bail!("login failed (HTTP 401). Check username/email and password.");
    }
    if response.status >= 400 {
        anyhow::bail!("login returned HTTP {}", response.status);
    }
    let token = response
        .header("Token")
        .context("login response missing Token header")?
        .to_string();
    let me: ApiUser = response.json()?;
    Ok(empty_account(site, token, me.into_user()))
}

/// Personal Access Token: skip the password POST, treat the token as Bearer.
pub fn login_with_token(transport: &dyn Transport, site: &str, token: &str) -> Result<Account> {
    let me: ApiUser = send(
        transport,
        site,
        Method::Get,
        "/api/v4/users/me",
        Some(token),
        Vec::new(),
    )?
    .json()?;
    Ok(empty_account(site, token.to_string(), me.into_user()))
}

/// Exchange the one-time server token from the browser redirect.
/// One POST: the route allows a burst of 1, and the token is not stored under
/// the client `desktop_token`, so polling that value cannot succeed.
pub fn login_with_desktop_token(
    transport: &dyn Transport,
    site: &str,
    server_token: &str,
) -> Result<Account> {
    let body = serde_json::to_vec(&serde_json::json!({
        "token": server_token,
        "deviceId": "",
        "device_id": "",
    }))?;
    let response = transport.send(&Request {
        method: Method::Post,
        url: format!("{site}/api/v4/users/login/desktop_token"),
        headers: vec![("Content-Type".into(), "application/json".into())],
        body,
    })?;
    if response.status == 401 || response.status == 403 {
        let id = serde_json::from_slice::<serde_json::Value>(&response.body)
            .ok()
            .and_then(|value| {
                value
                    .get("id")
                    .and_then(|id| id.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if id.contains("not_oauth_or_saml") {
            anyhow::bail!(
                "That sign-in link only works for SSO accounts. Use password sign-in instead."
            );
        }
        anyhow::bail!(
            "That sign-in link was already used or has expired. Sign in with the browser again. If a prompt offers to open Mattermost, cancel it and paste the page address here."
        );
    }
    if response.status >= 400 {
        anyhow::bail!("desktop sign-in returned HTTP {}", response.status);
    }
    let token = response
        .header("Token")
        .context("desktop sign-in did not return a session")?
        .to_string();
    let me: ApiUser = response.json()?;
    Ok(empty_account(site, token, me.into_user()))
}

pub fn bootstrap(transport: &dyn Transport, account: &mut Account) -> Result<()> {
    let site = account.site_url.clone();
    let me: ApiUser = send(
        transport,
        &site,
        Method::Get,
        "/api/v4/users/me",
        Some(&account.token),
        Vec::new(),
    )?
    .json()?;
    account.me = me.into_user();
    if let Ok(config) = fetch_client_config(transport, &site) {
        account.websocket_url = websocket_url(&site, &config.websocket_url);
    }

    let teams: Vec<ApiTeam> = send(
        transport,
        &site,
        Method::Get,
        "/api/v4/users/me/teams",
        Some(&account.token),
        Vec::new(),
    )?
    .json()?;
    anyhow::ensure!(!teams.is_empty(), "no teams for user");
    account.teams = teams
        .into_iter()
        .map(|team| Team {
            id: team.id,
            name: team.name,
            display_name: team.display_name,
        })
        .collect();
    load_team(transport, account, 0)
}

/// Channels, categories, and mention counts for one team. DMs come back inside
/// that team's sidebar categories.
pub fn load_team(transport: &dyn Transport, account: &mut Account, index: usize) -> Result<()> {
    let team = account.teams.get(index).context("no such team")?.clone();
    account.team = team;
    let site = account.site_url.clone();
    let channels: Vec<ApiChannel> = send(
        transport,
        &site,
        Method::Get,
        &format!("/api/v4/users/me/teams/{}/channels", account.team.id),
        Some(&account.token),
        Vec::new(),
    )?
    .json()?;

    let categories: CategoryList = send(
        transport,
        &site,
        Method::Get,
        &format!(
            "/api/v4/users/{}/teams/{}/channels/categories",
            account.me.id, account.team.id
        ),
        Some(&account.token),
        Vec::new(),
    )?
    .json()?;

    let mut favorite_ids = Vec::new();
    for category in &categories.categories {
        if category.kind == "favorites" {
            favorite_ids = category.channel_ids.clone();
        }
    }

    let mut users = fetch_users(transport, &site, &account.token, &user_ids_from(&channels))?;
    for user in &mut users {
        if user.avatar.is_none()
            && let Ok(bytes) = fetch_avatar(transport, &site, &account.token, &user.id)
        {
            user.avatar = Some(bytes);
        }
    }
    account.users = users.clone();
    let mut rooms = Vec::new();
    for channel in channels {
        let kind = match channel.kind.as_str() {
            "P" => RoomKind::Private,
            "D" | "G" => RoomKind::Direct,
            _ => RoomKind::Public,
        };
        let display_name = if kind == RoomKind::Direct {
            dm_display(&channel.name, &account.me.id, &account.users)
        } else if channel.display_name.is_empty() {
            channel.name.clone()
        } else {
            channel.display_name.clone()
        };
        let favorite = favorite_ids.iter().any(|id| id == &channel.id);
        rooms.push(Room {
            id: channel.id,
            name: channel.name,
            display_name,
            kind,
            favorite,
            mentions: 0,
            unread: false,
        });
    }
    account.sidebar = Sidebar { rooms };
    apply_members(transport, account);
    Ok(())
}

pub fn next_team_index(account: &Account) -> Option<usize> {
    if account.teams.len() < 2 {
        return None;
    }
    let current = account
        .teams
        .iter()
        .position(|team| team.id == account.team.id)
        .unwrap_or(0);
    Some((current + 1) % account.teams.len())
}

fn user_ids_from(channels: &[ApiChannel]) -> Vec<String> {
    let mut ids = Vec::new();
    for channel in channels {
        if channel.kind == "D" {
            for part in channel.name.split("__") {
                if !part.is_empty() && !ids.iter().any(|id| id == part) {
                    ids.push(part.to_string());
                }
            }
        }
    }
    ids
}

fn dm_display(name: &str, me: &str, users: &[User]) -> String {
    let other = name.split("__").find(|id| *id != me).unwrap_or(name);
    users
        .iter()
        .find(|user| user.id == other)
        .map(|user| user.display_name.clone())
        .unwrap_or_else(|| other.to_string())
}

fn fetch_users(
    transport: &dyn Transport,
    site: &str,
    token: &str,
    extra_ids: &[String],
) -> Result<Vec<User>> {
    if extra_ids.is_empty() {
        return Ok(Vec::new());
    }
    let body = serde_json::to_vec(&extra_ids)?;
    let parsed: Vec<ApiUser> = send(
        transport,
        site,
        Method::Post,
        "/api/v4/users/ids",
        Some(token),
        body,
    )?
    .json()?;
    Ok(parsed.into_iter().map(ApiUser::into_user).collect())
}

fn fetch_avatar(
    transport: &dyn Transport,
    site: &str,
    token: &str,
    user_id: &str,
) -> Result<Vec<u8>> {
    let response = send(
        transport,
        site,
        Method::Get,
        &format!("/api/v4/users/{user_id}/image"),
        Some(token),
        Vec::new(),
    )?;
    Ok(response.body)
}

pub fn fetch_posts(
    transport: &dyn Transport,
    site: &str,
    token: &str,
    channel_id: &str,
    users: &mut Vec<User>,
) -> Result<Vec<Message>> {
    let list: PostList = send(
        transport,
        site,
        Method::Get,
        &format!("/api/v4/channels/{channel_id}/posts?per_page=60"),
        Some(token),
        Vec::new(),
    )?
    .json()?;
    let mut messages = Vec::new();
    for id in list.order.iter().rev() {
        let Some(value) = list.posts.get(id) else {
            continue;
        };
        let user_id = value
            .get("user_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let source = value
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let create_at = value.get("create_at").and_then(|v| v.as_i64()).unwrap_or(0);
        if !users.iter().any(|user| user.id == user_id) {
            let fetched = fetch_users(transport, site, token, std::slice::from_ref(&user_id))?;
            for user in fetched {
                if !users.iter().any(|existing| existing.id == user.id) {
                    users.push(user);
                }
            }
        }
        let author = users
            .iter()
            .find(|user| user.id == user_id)
            .cloned()
            .unwrap_or(User {
                id: user_id.clone(),
                username: user_id.clone(),
                display_name: user_id.clone(),
                avatar: None,
            });
        let portrait = match &author.avatar {
            Some(bytes) => Some(Portrait {
                user_id: author.id.clone(),
                bytes: bytes.clone(),
            }),
            None => match fetch_avatar(transport, site, token, &author.id) {
                Ok(bytes) => {
                    if let Some(user) = users.iter_mut().find(|user| user.id == author.id) {
                        user.avatar = Some(bytes.clone());
                    }
                    Some(Portrait {
                        user_id: author.id.clone(),
                        bytes,
                    })
                }
                Err(_) => None,
            },
        };
        messages.push(Message {
            id: id.clone(),
            channel_id: channel_id.to_string(),
            user_id: author.id.clone(),
            author_name: author.display_name,
            body_source: source.clone(),
            body: expand_shortcodes(&source),
            create_at,
            portrait,
        });
    }
    Ok(messages)
}

/// Cache-first page. HTTP only on a cold channel. Always records `loaded_on`.
pub fn page_messages(
    transport: &dyn Transport,
    cache: &Cache,
    site: &str,
    token: &str,
    users: &mut Vec<User>,
    channel_id: &str,
) -> Result<crate::core::MessagePage> {
    let loaded_on = std::thread::current().id();
    if let Some(messages) = cache.page(channel_id)? {
        return Ok(crate::core::MessagePage {
            channel_id: channel_id.to_string(),
            messages,
            from_cache: true,
            loaded_on,
        });
    }
    let messages = fetch_posts(transport, site, token, channel_id, users)?;
    for user in users.iter() {
        cache.upsert_user(user)?;
    }
    cache.replace_messages(channel_id, &messages)?;
    Ok(crate::core::MessagePage {
        channel_id: channel_id.to_string(),
        messages,
        from_cache: false,
        loaded_on,
    })
}

pub fn set_favorite(
    account: &mut Account,
    cache: &Cache,
    channel_id: &str,
    favorite: bool,
) -> bool {
    let changed = account.sidebar.set_favorite(channel_id, favorite);
    if changed {
        let _ = cache.set_favorite(channel_id, favorite);
        let _ = cache.replace_sidebar(&account.sidebar);
    }
    changed
}

pub fn persist_account(cache: &Cache, account: &Account, users: &[User]) -> Result<()> {
    cache.replace_sidebar(&account.sidebar)?;
    cache.upsert_user(&account.me)?;
    for user in users {
        cache.upsert_user(user)?;
    }
    Ok(())
}

/// Always hit the network. Used once when the socket connects, and once when a
/// channel's cache was written by a previous run. Not a timer.
pub fn reload_posts(
    transport: &dyn Transport,
    cache: &Cache,
    site: &str,
    token: &str,
    users: &mut Vec<User>,
    channel_id: &str,
) -> Result<crate::core::MessagePage> {
    let messages = fetch_posts(transport, site, token, channel_id, users)?;
    for user in users.iter() {
        cache.upsert_user(user)?;
    }
    cache.replace_messages(channel_id, &messages)?;
    Ok(crate::core::MessagePage {
        channel_id: channel_id.to_string(),
        messages,
        from_cache: false,
        loaded_on: std::thread::current().id(),
    })
}

pub fn view_channel(
    transport: &dyn Transport,
    site: &str,
    token: &str,
    channel_id: &str,
    previous: Option<&str>,
) -> Result<()> {
    let mut payload = serde_json::json!({ "channel_id": channel_id });
    if let Some(previous) = previous.filter(|id| !id.is_empty()) {
        payload["prev_channel_id"] = serde_json::Value::String(previous.to_string());
    }
    send(
        transport,
        site,
        Method::Post,
        "/api/v4/channels/members/me/view",
        Some(token),
        serde_json::to_vec(&payload)?,
    )?;
    Ok(())
}

pub fn create_post(
    transport: &dyn Transport,
    site: &str,
    token: &str,
    users: &mut Vec<User>,
    channel_id: &str,
    text: &str,
    pending_id: &str,
) -> Result<Message> {
    let body = serde_json::to_vec(&serde_json::json!({
        "channel_id": channel_id,
        "message": text,
        "pending_post_id": pending_id,
    }))?;
    let value: serde_json::Value = send(
        transport,
        site,
        Method::Post,
        "/api/v4/posts",
        Some(token),
        body,
    )?
    .json()?;
    message_from_value(transport, site, token, users, &value, "")
}

pub fn patch_post(
    transport: &dyn Transport,
    site: &str,
    token: &str,
    users: &mut Vec<User>,
    post_id: &str,
    text: &str,
) -> Result<Message> {
    let body = serde_json::to_vec(&serde_json::json!({ "message": text }))?;
    let value: serde_json::Value = send(
        transport,
        site,
        Method::Put,
        &format!("/api/v4/posts/{post_id}/patch"),
        Some(token),
        body,
    )?
    .json()?;
    message_from_value(transport, site, token, users, &value, "")
}

pub fn delete_post(
    transport: &dyn Transport,
    site: &str,
    token: &str,
    post_id: &str,
) -> Result<()> {
    send(
        transport,
        site,
        Method::Delete,
        &format!("/api/v4/posts/{post_id}"),
        Some(token),
        Vec::new(),
    )?;
    Ok(())
}

pub fn message_from_post(
    transport: &dyn Transport,
    site: &str,
    token: &str,
    users: &mut Vec<User>,
    post: &ParsedPost,
) -> Result<Message> {
    let user_id = post.user_id.as_str();
    if !user_id.is_empty()
        && !users.iter().any(|user| user.id == user_id)
        && let Ok(fetched) =
            fetch_users(transport, site, token, std::slice::from_ref(&post.user_id))
    {
        for user in fetched {
            if !users.iter().any(|existing| existing.id == user.id) {
                users.push(user);
            }
        }
    }
    let mut author = users
        .iter()
        .find(|user| user.id == user_id)
        .cloned()
        .unwrap_or(User {
            id: user_id.to_string(),
            username: user_id.to_string(),
            display_name: if post.sender_name.is_empty() {
                user_id.to_string()
            } else {
                post.sender_name.clone()
            },
            avatar: None,
        });
    if author.avatar.is_none()
        && !author.id.is_empty()
        && let Ok(bytes) = fetch_avatar(transport, site, token, &author.id)
    {
        author.avatar = Some(bytes);
        if let Some(user) = users.iter_mut().find(|user| user.id == author.id) {
            user.avatar = author.avatar.clone();
        }
    }
    let portrait = author.avatar.as_ref().map(|bytes| Portrait {
        user_id: author.id.clone(),
        bytes: bytes.clone(),
    });
    Ok(Message {
        id: post.id.clone(),
        channel_id: post.channel_id.clone(),
        user_id: author.id,
        author_name: author.display_name,
        body_source: post.message.clone(),
        body: expand_shortcodes(&post.message),
        create_at: post.create_at,
        portrait,
    })
}

#[derive(Clone, Debug)]
pub struct ParsedPost {
    pub id: String,
    pub channel_id: String,
    pub user_id: String,
    pub message: String,
    pub create_at: i64,
    pub pending_post_id: String,
    pub sender_name: String,
}

#[derive(Debug)]
pub enum Incoming {
    AuthOk,
    AuthFail,
    Posted(ParsedPost),
    Edited(ParsedPost),
    Deleted { id: String, channel_id: String },
}

/// Mattermost websocket text frame. `posted` carries `data.post` as a JSON string.
pub fn parse_ws_message(text: &str) -> Option<Incoming> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("seq_reply").is_some() {
        let status = value.get("status").and_then(|status| status.as_str())?;
        return Some(if status.eq_ignore_ascii_case("ok") {
            Incoming::AuthOk
        } else {
            Incoming::AuthFail
        });
    }
    let event = value.get("event").and_then(|event| event.as_str())?;
    let data = value.get("data")?;
    match event {
        "posted" => parsed_post(data).map(Incoming::Posted),
        "post_edited" => parsed_post(data).map(Incoming::Edited),
        "post_deleted" => {
            let post = parsed_post(data)?;
            Some(Incoming::Deleted {
                id: post.id,
                channel_id: post.channel_id,
            })
        }
        _ => None,
    }
}

pub fn mentions_me(body: &str, me: &User) -> bool {
    let lower = body.to_ascii_lowercase();
    let at = format!("@{}", me.username.to_ascii_lowercase());
    lower.contains(&at)
        || lower.contains("@all")
        || lower.contains("@channel")
        || lower.contains("@here")
}

pub fn local_id() -> String {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("system rng");
    let mut out = String::from("local-");
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Debug, Default)]
pub struct ClientConfig {
    pub site_name: String,
    pub websocket_url: String,
    pub email: bool,
    pub username: bool,
    pub ldap: bool,
    pub gitlab: bool,
    pub google: bool,
    pub office365: bool,
    pub openid: bool,
    pub saml: bool,
}

impl ClientConfig {
    pub fn sso(&self) -> bool {
        self.gitlab || self.google || self.office365 || self.openid || self.saml
    }

    pub fn providers(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.saml {
            names.push("SAML");
        }
        if self.gitlab {
            names.push("GitLab");
        }
        if self.google {
            names.push("Google");
        }
        if self.office365 {
            names.push("Entra ID");
        }
        if self.openid {
            names.push("OpenID");
        }
        names
    }
}

pub fn fetch_client_config(transport: &dyn Transport, site: &str) -> Result<ClientConfig> {
    let map: serde_json::Map<String, serde_json::Value> = send(
        transport,
        site,
        Method::Get,
        "/api/v4/config/client",
        None,
        Vec::new(),
    )?
    .json()?;
    Ok(ClientConfig {
        site_name: text_flag(&map, "SiteName"),
        websocket_url: text_flag(&map, "WebsocketURL"),
        email: bool_flag(&map, "EnableSignInWithEmail"),
        username: bool_flag(&map, "EnableSignInWithUsername"),
        ldap: bool_flag(&map, "EnableLdap"),
        gitlab: bool_flag(&map, "EnableSignUpWithGitLab"),
        google: bool_flag(&map, "EnableSignUpWithGoogle"),
        office365: bool_flag(&map, "EnableSignUpWithOffice365"),
        openid: bool_flag(&map, "EnableSignUpWithOpenId"),
        saml: bool_flag(&map, "EnableSaml"),
    })
}

/// `wss://` from the Site URL, or `WebsocketURL` from the client config.
/// The path is `/api/v4/websocket` when the server only publishes a base.
pub fn websocket_url(site: &str, configured: &str) -> String {
    let configured = configured.trim().trim_end_matches('/');
    if let Some(rest) = configured.strip_prefix("wss://") {
        if rest.contains("/api/v4/websocket") {
            return configured.to_string();
        }
        if rest.is_empty() {
            return String::new();
        }
        return format!("{configured}/api/v4/websocket");
    }
    let rest = site.trim_end_matches('/').trim_start_matches("https://");
    format!("wss://{rest}/api/v4/websocket")
}

fn bool_flag(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    match map.get(key) {
        Some(serde_json::Value::Bool(value)) => *value,
        Some(serde_json::Value::String(value)) => value == "true",
        _ => false,
    }
}

fn text_flag(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    map.get(key)
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string()
}

#[derive(serde::Deserialize)]
struct ApiMember {
    channel_id: String,
    #[serde(default)]
    mention_count: u32,
}

fn apply_members(transport: &dyn Transport, account: &mut Account) {
    let path = format!(
        "/api/v4/users/me/teams/{}/channels/members",
        account.team.id
    );
    let Ok(response) = send(
        transport,
        &account.site_url,
        Method::Get,
        &path,
        Some(&account.token),
        Vec::new(),
    ) else {
        return;
    };
    let Ok(members) = response.json::<Vec<ApiMember>>() else {
        return;
    };
    for member in members {
        if let Some(room) = account
            .sidebar
            .rooms
            .iter_mut()
            .find(|room| room.id == member.channel_id)
        {
            room.mentions = member.mention_count;
        }
    }
}

fn parsed_post(data: &serde_json::Value) -> Option<ParsedPost> {
    let sender_name = data
        .get("sender_name")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let post = match data.get("post")? {
        serde_json::Value::String(text) => serde_json::from_str(text).ok()?,
        other => other.clone(),
    };
    post_from_value(&post, &sender_name)
}

fn post_from_value(value: &serde_json::Value, sender_name: &str) -> Option<ParsedPost> {
    Some(ParsedPost {
        id: value.get("id")?.as_str()?.to_string(),
        channel_id: value.get("channel_id")?.as_str()?.to_string(),
        user_id: value
            .get("user_id")
            .and_then(|user| user.as_str())
            .unwrap_or("")
            .to_string(),
        message: value
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("")
            .to_string(),
        create_at: value
            .get("create_at")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        pending_post_id: value
            .get("pending_post_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        sender_name: sender_name.to_string(),
    })
}

fn message_from_value(
    transport: &dyn Transport,
    site: &str,
    token: &str,
    users: &mut Vec<User>,
    value: &serde_json::Value,
    sender_name: &str,
) -> Result<Message> {
    let post = post_from_value(value, sender_name).context("post missing id")?;
    message_from_post(transport, site, token, users, &post)
}
