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
    pub team: Team,
    pub sidebar: Sidebar,
    pub users: Vec<User>,
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
        bail!(
            "{} {path} returned HTTP {}",
            method.as_str(),
            response.status
        );
    }
    Ok(response)
}

fn empty_account(site: &str, token: String, me: User) -> Account {
    Account {
        site_url: site.to_string(),
        token,
        me,
        team: Team {
            id: String::new(),
            name: String::new(),
            display_name: String::new(),
        },
        sidebar: Sidebar::default(),
        users: Vec::new(),
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

    let teams: Vec<ApiTeam> = send(
        transport,
        &site,
        Method::Get,
        "/api/v4/users/me/teams",
        Some(&account.token),
        Vec::new(),
    )?
    .json()?;
    let team = teams.into_iter().next().context("no teams for user")?;
    account.team = Team {
        id: team.id.clone(),
        name: team.name,
        display_name: team.display_name,
    };

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
        });
    }
    account.sidebar = Sidebar { rooms };
    Ok(())
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
        &format!("/api/v4/channels/{channel_id}/posts"),
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
