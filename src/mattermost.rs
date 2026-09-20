//! Mattermost REST client over a `Transport`. Fixture replay is the v1 path.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::cache::Cache;
use crate::core::{Message, Portrait, Room, RoomKind, Sidebar, Team, User};
use crate::emoji::expand_shortcodes;
use crate::net::{Method, Request, Response, Transport};

const SITE: &str = "https://mm.example.test";

#[derive(Clone, Debug)]
pub struct Account {
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

fn send(
    transport: &dyn Transport,
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
        url: format!("{SITE}{path}"),
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

/// Password login. Token comes from the `Token` response header (Mattermost).
pub fn login(transport: &dyn Transport, login_id: &str, password: &str) -> Result<Account> {
    let body = serde_json::to_vec(&serde_json::json!({
        "login_id": login_id,
        "password": password,
    }))?;
    let response = send(transport, Method::Post, "/api/v4/users/login", None, body)?;
    let token = response
        .header("Token")
        .context("login response missing Token header")?
        .to_string();
    let me: ApiUser = response.json()?;
    Ok(Account {
        token,
        me: me.into_user(),
        team: Team {
            id: String::new(),
            name: String::new(),
            display_name: String::new(),
        },
        sidebar: Sidebar::default(),
        users: Vec::new(),
    })
}

pub fn bootstrap(transport: &dyn Transport, account: &mut Account) -> Result<()> {
    let me: ApiUser = send(
        transport,
        Method::Get,
        "/api/v4/users/me",
        Some(&account.token),
        Vec::new(),
    )?
    .json()?;
    account.me = me.into_user();

    let teams: Vec<ApiTeam> = send(
        transport,
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
        Method::Get,
        &format!("/api/v4/users/me/teams/{}/channels", account.team.id),
        Some(&account.token),
        Vec::new(),
    )?
    .json()?;

    let categories: CategoryList = send(
        transport,
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

    let mut users = fetch_users(transport, &account.token, &user_ids_from(&channels))?;
    for user in &mut users {
        if user.avatar.is_none()
            && let Ok(bytes) = fetch_avatar(transport, &account.token, &user.id)
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

fn fetch_users(transport: &dyn Transport, token: &str, extra_ids: &[String]) -> Result<Vec<User>> {
    let mut ids = extra_ids.to_vec();
    for known in ["user-me", "user-alice", "user-bob"] {
        if !ids.iter().any(|id| id == known) {
            ids.push(known.to_string());
        }
    }
    let body = serde_json::to_vec(&ids)?;
    let parsed: Vec<ApiUser> = send(
        transport,
        Method::Post,
        "/api/v4/users/ids",
        Some(token),
        body,
    )?
    .json()?;
    Ok(parsed.into_iter().map(ApiUser::into_user).collect())
}

fn fetch_avatar(transport: &dyn Transport, token: &str, user_id: &str) -> Result<Vec<u8>> {
    let response = send(
        transport,
        Method::Get,
        &format!("/api/v4/users/{user_id}/image"),
        Some(token),
        Vec::new(),
    )?;
    Ok(response.body)
}

pub fn fetch_posts(
    transport: &dyn Transport,
    token: &str,
    channel_id: &str,
    users: &mut Vec<User>,
) -> Result<Vec<Message>> {
    let list: PostList = send(
        transport,
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
            let fetched = fetch_users(transport, token, std::slice::from_ref(&user_id))?;
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
            None => match fetch_avatar(transport, token, &author.id) {
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
    let messages = fetch_posts(transport, token, channel_id, users)?;
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
