//! Background session: cache, HTTP, and the websocket never run on the UI thread.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use crate::cache::Cache;
use crate::core::{Link, Message, MessagePage, Sidebar};
use crate::handoff::{self, Handoff};
use crate::mattermost::{self, Account};
use crate::net::Transport;
use crate::ws::{self, LiveNote};

pub enum Command {
    Login {
        site: String,
        login_id: String,
        password: String,
        totp: String,
        pat: String,
    },
    BrowserLogin {
        site: String,
    },
    CompleteHandoff {
        raw: String,
        fallback_site: String,
    },
    LoadHistory {
        channel_id: String,
    },
    Send {
        channel_id: String,
        text: String,
        editing: Option<String>,
    },
    Delete {
        post_id: String,
    },
    SwitchTeam,
    SetFavorite {
        channel_id: String,
        favorite: bool,
    },
    Live(LiveNote),
    Shutdown,
}

pub enum Event {
    Ready {
        account: Account,
    },
    Account {
        account: Account,
    },
    Sidebar {
        sidebar: Sidebar,
    },
    Messages {
        page: MessagePage,
    },
    Upsert {
        message: Message,
    },
    Removed {
        channel_id: String,
        post_id: String,
    },
    Link {
        state: Link,
        updated: bool,
    },
    WaitingBrowser {
        providers: Vec<String>,
    },
    /// `login` is a sign-in failure. Anything else is a chat action the user can retry.
    Error {
        message: String,
        login: bool,
    },
    AuthExpired,
}

pub struct Session {
    cmd: Sender<Command>,
    ev: Receiver<Event>,
    worker: Option<JoinHandle<()>>,
    worker_thread: thread::ThreadId,
}

impl Session {
    pub fn spawn(transport: Arc<dyn Transport>, cache: Cache) -> Self {
        Self::spawn_waking(transport, cache, Arc::new(|| {}))
    }

    pub fn spawn_waking(
        transport: Arc<dyn Transport>,
        cache: Cache,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker_tx = cmd_tx.clone();
        let worker = thread::Builder::new()
            .name("comport-session".into())
            .spawn(move || {
                let _ = ready_tx.send(thread::current().id());
                worker_loop(transport, cache, worker_tx, cmd_rx, ev_tx, wake);
            })
            .expect("spawn session worker");
        let worker_thread = ready_rx.recv().expect("worker thread id");
        Self {
            cmd: cmd_tx,
            ev: ev_rx,
            worker: Some(worker),
            worker_thread,
        }
    }

    pub fn worker_thread(&self) -> thread::ThreadId {
        self.worker_thread
    }

    pub fn login(&self, site: &str, login_id: &str, password: &str) -> Result<()> {
        self.login_full(site, login_id, password, "", "")
    }

    pub fn login_full(
        &self,
        site: &str,
        login_id: &str,
        password: &str,
        totp: &str,
        pat: &str,
    ) -> Result<()> {
        self.cmd
            .send(Command::Login {
                site: site.into(),
                login_id: login_id.into(),
                password: password.into(),
                totp: totp.into(),
                pat: pat.into(),
            })
            .context("session closed")
    }

    pub fn browser_login(&self, site: &str) -> Result<()> {
        self.cmd
            .send(Command::BrowserLogin { site: site.into() })
            .context("session closed")
    }

    pub fn complete_handoff(&self, raw: &str, fallback_site: &str) -> Result<()> {
        self.cmd
            .send(Command::CompleteHandoff {
                raw: raw.into(),
                fallback_site: fallback_site.into(),
            })
            .context("session closed")
    }

    pub fn load_history(&self, channel_id: &str) -> Result<()> {
        self.cmd
            .send(Command::LoadHistory {
                channel_id: channel_id.into(),
            })
            .context("session closed")
    }

    pub fn send_message(
        &self,
        channel_id: &str,
        text: &str,
        editing: Option<String>,
    ) -> Result<()> {
        self.cmd
            .send(Command::Send {
                channel_id: channel_id.into(),
                text: text.into(),
                editing,
            })
            .context("session closed")
    }

    pub fn delete_message(&self, post_id: &str) -> Result<()> {
        self.cmd
            .send(Command::Delete {
                post_id: post_id.into(),
            })
            .context("session closed")
    }

    pub fn switch_team(&self) -> Result<()> {
        self.cmd.send(Command::SwitchTeam).context("session closed")
    }

    pub fn set_favorite(&self, channel_id: &str, favorite: bool) -> Result<()> {
        self.cmd
            .send(Command::SetFavorite {
                channel_id: channel_id.into(),
                favorite,
            })
            .context("session closed")
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event> {
        match self.ev.recv_timeout(timeout) {
            Ok(event) => Ok(event),
            Err(RecvTimeoutError::Timeout) => Err(anyhow!("timed out waiting for session event")),
            Err(RecvTimeoutError::Disconnected) => Err(anyhow!("session worker disconnected")),
        }
    }

    pub fn try_recv(&self) -> Result<Option<Event>> {
        match self.ev.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow!("session worker disconnected")),
        }
    }

    /// Test helper: wait until a Messages event arrives.
    pub fn wait_page(&self, timeout: Duration) -> Result<MessagePage> {
        loop {
            match self.recv_timeout(timeout)? {
                Event::Messages { page } => return Ok(page),
                Event::Error { message, .. } => anyhow::bail!(message),
                _ => {}
            }
        }
    }

    pub fn wait_ready(&self, timeout: Duration) -> Result<Account> {
        loop {
            match self.recv_timeout(timeout)? {
                Event::Ready { account } => return Ok(account),
                Event::Error { message, .. } => anyhow::bail!(message),
                _ => {}
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.cmd.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Realtime {
    fresh: HashSet<String>,
    selected: Option<String>,
    previous: Option<String>,
    socket: Option<ws::Socket>,
    live: bool,
}

impl Realtime {
    fn new() -> Self {
        Self {
            fresh: HashSet::new(),
            selected: None,
            previous: None,
            socket: None,
            live: false,
        }
    }

    fn reset(&mut self) {
        self.socket.take();
        self.fresh.clear();
        self.selected = None;
        self.previous = None;
        self.live = false;
    }
}

fn worker_loop(
    transport: Arc<dyn Transport>,
    cache: Cache,
    cmd_tx: Sender<Command>,
    cmd: Receiver<Command>,
    ev: Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    let emit = |event: Event| {
        let _ = ev.send(event);
        wake();
    };
    let mut account: Option<Account> = None;
    let mut realtime = Realtime::new();
    while let Ok(command) = cmd.recv() {
        match command {
            Command::Shutdown => {
                realtime.reset();
                break;
            }
            Command::Login {
                site,
                login_id,
                password,
                totp,
                pat,
            } => match login_and_bootstrap(
                transport.as_ref(),
                &cache,
                &site,
                &login_id,
                &password,
                &totp,
                &pat,
            ) {
                Ok(ready) => {
                    adopt(&transport, &cmd_tx, &mut account, &mut realtime, ready);
                    if let Some(ready) = account.clone() {
                        emit(Event::Ready { account: ready });
                    }
                }
                Err(error) => emit(Event::Error {
                    message: format!("{error:#}"),
                    login: true,
                }),
            },
            Command::BrowserLogin { site } => match open_browser_login(transport.as_ref(), &site) {
                Ok(providers) => emit(Event::WaitingBrowser { providers }),
                Err(error) => emit(Event::Error {
                    message: format!("{error:#}"),
                    login: true,
                }),
            },
            Command::CompleteHandoff { raw, fallback_site } => {
                match finish_handoff(transport.as_ref(), &cache, &raw, &fallback_site) {
                    Ok(ready) => {
                        adopt(&transport, &cmd_tx, &mut account, &mut realtime, ready);
                        if let Some(ready) = account.clone() {
                            emit(Event::Ready { account: ready });
                        }
                    }
                    Err(error) => emit(Event::Error {
                        message: format!("{error:#}"),
                        login: true,
                    }),
                }
            }
            Command::LoadHistory { channel_id } => {
                let Some(account) = account.as_mut() else {
                    emit(Event::Error {
                        message: "not logged in".into(),
                        login: false,
                    });
                    continue;
                };
                match load_history(
                    transport.as_ref(),
                    &cache,
                    account,
                    &mut realtime,
                    &channel_id,
                ) {
                    Ok(pages) => {
                        let sidebar = account.sidebar.clone();
                        for page in pages {
                            emit(Event::Messages { page });
                        }
                        emit(Event::Sidebar { sidebar });
                    }
                    Err(error) => emit(Event::Error {
                        message: format!("{error:#}"),
                        login: false,
                    }),
                }
            }
            Command::Send {
                channel_id,
                text,
                editing,
            } => {
                let Some(account) = account.as_mut() else {
                    continue;
                };
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                if let Some(post_id) = editing.as_deref() {
                    match mattermost::patch_post(
                        transport.as_ref(),
                        &account.site_url,
                        &account.token,
                        &mut account.users,
                        post_id,
                        text,
                    ) {
                        Ok(message) => {
                            remember_user(&cache, account, &message.user_id);
                            publish(&cache, &emit, &message);
                        }
                        Err(error) => emit(Event::Error {
                            message: format!("{error:#}"),
                            login: false,
                        }),
                    }
                    continue;
                }
                let pending = mattermost::local_id();
                let optimistic = optimistic_message(account, &channel_id, text, &pending);
                publish(&cache, &emit, &optimistic);
                match mattermost::create_post(
                    transport.as_ref(),
                    &account.site_url,
                    &account.token,
                    &mut account.users,
                    &channel_id,
                    text,
                    &pending,
                ) {
                    Ok(message) => {
                        let _ = cache.delete_message(&pending);
                        emit(Event::Removed {
                            channel_id: channel_id.clone(),
                            post_id: pending,
                        });
                        remember_user(&cache, account, &message.user_id);
                        publish(&cache, &emit, &message);
                    }
                    Err(error) => {
                        let _ = cache.delete_message(&pending);
                        emit(Event::Removed {
                            channel_id,
                            post_id: pending,
                        });
                        emit(Event::Error {
                            message: format!("{error:#}"),
                            login: false,
                        });
                    }
                }
            }
            Command::Delete { post_id } => {
                let Some(account) = account.as_mut() else {
                    continue;
                };
                let channel_id = realtime.selected.clone().unwrap_or_default();
                match mattermost::delete_post(
                    transport.as_ref(),
                    &account.site_url,
                    &account.token,
                    &post_id,
                ) {
                    Ok(()) => {
                        let _ = cache.delete_message(&post_id);
                        emit(Event::Removed {
                            channel_id,
                            post_id,
                        });
                    }
                    Err(error) => emit(Event::Error {
                        message: format!("{error:#}"),
                        login: false,
                    }),
                }
            }
            Command::SwitchTeam => {
                let Some(account) = account.as_mut() else {
                    continue;
                };
                let Some(index) = mattermost::next_team_index(account) else {
                    continue;
                };
                match mattermost::load_team(transport.as_ref(), account, index) {
                    Ok(()) => {
                        let _ = mattermost::persist_account(&cache, account, &account.users);
                        realtime.fresh.clear();
                        realtime.selected = None;
                        realtime.previous = None;
                        emit(Event::Account {
                            account: account.clone(),
                        });
                    }
                    Err(error) => emit(Event::Error {
                        message: format!("{error:#}"),
                        login: false,
                    }),
                }
            }
            Command::SetFavorite {
                channel_id,
                favorite,
            } => {
                let Some(account) = account.as_mut() else {
                    emit(Event::Error {
                        message: "not logged in".into(),
                        login: false,
                    });
                    continue;
                };
                mattermost::set_favorite(account, &cache, &channel_id, favorite);
                emit(Event::Sidebar {
                    sidebar: account.sidebar.clone(),
                });
            }
            Command::Live(note) => handle_live(
                transport.as_ref(),
                &cache,
                &mut account,
                &mut realtime,
                note,
                &emit,
            ),
        }
    }
}

fn adopt(
    transport: &Arc<dyn Transport>,
    cmd_tx: &Sender<Command>,
    account: &mut Option<Account>,
    realtime: &mut Realtime,
    ready: Account,
) {
    realtime.reset();
    if transport.live() {
        let url = ready.websocket_url.clone();
        let token = ready.token.clone();
        let tx = cmd_tx.clone();
        realtime.socket = Some(ws::Socket::start(url, token, move |note| {
            let _ = tx.send(Command::Live(note));
        }));
    }
    *account = Some(ready);
}

fn open_browser_login(transport: &dyn Transport, site: &str) -> Result<Vec<String>> {
    let site = mattermost::normalize_site(site)?;
    let config = mattermost::fetch_client_config(transport, &site).ok();
    let providers = config
        .as_ref()
        .map(|config| {
            config
                .providers()
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let url = handoff::browser_login_url(&site, &handoff::desktop_token());
    handoff::open_browser(&url)?;
    Ok(providers)
}

fn finish_handoff(
    transport: &dyn Transport,
    cache: &Cache,
    raw: &str,
    fallback_site: &str,
) -> Result<Account> {
    let Handoff {
        site_url,
        server_token,
    } = handoff::parse_handoff(raw, fallback_site)?;
    let mut account = mattermost::login_with_desktop_token(transport, &site_url, &server_token)?;
    mattermost::bootstrap(transport, &mut account)?;
    mattermost::persist_account(cache, &account, &account.users)?;
    Ok(account)
}

fn login_and_bootstrap(
    transport: &dyn Transport,
    cache: &Cache,
    site: &str,
    login_id: &str,
    password: &str,
    totp: &str,
    pat: &str,
) -> Result<Account> {
    let site = mattermost::normalize_site(site)?;
    let mut account = if !pat.trim().is_empty() {
        mattermost::login_with_token(transport, &site, pat.trim())?
    } else {
        mattermost::login(
            transport,
            &site,
            login_id,
            password,
            (!totp.is_empty()).then_some(totp),
        )?
    };
    mattermost::bootstrap(transport, &mut account)?;
    mattermost::persist_account(cache, &account, &account.users)?;
    Ok(account)
}

fn load_history(
    transport: &dyn Transport,
    cache: &Cache,
    account: &mut Account,
    realtime: &mut Realtime,
    channel_id: &str,
) -> Result<Vec<MessagePage>> {
    let mut pages = Vec::new();
    let page = mattermost::page_messages(
        transport,
        cache,
        &account.site_url,
        &account.token,
        &mut account.users,
        channel_id,
    )?;
    let stale = page.from_cache && !realtime.fresh.contains(channel_id);
    if !page.from_cache {
        realtime.fresh.insert(channel_id.to_string());
        let _ = mattermost::view_channel(
            transport,
            &account.site_url,
            &account.token,
            channel_id,
            realtime.previous.as_deref(),
        );
    }
    pages.push(page);
    if stale {
        let page = mattermost::reload_posts(
            transport,
            cache,
            &account.site_url,
            &account.token,
            &mut account.users,
            channel_id,
        )?;
        realtime.fresh.insert(channel_id.to_string());
        let _ = mattermost::view_channel(
            transport,
            &account.site_url,
            &account.token,
            channel_id,
            realtime.previous.as_deref(),
        );
        pages.push(page);
    }
    realtime.previous = realtime.selected.clone();
    realtime.selected = Some(channel_id.to_string());
    if let Some(room) = account
        .sidebar
        .rooms
        .iter_mut()
        .find(|room| room.id == channel_id)
    {
        room.unread = false;
        room.mentions = 0;
    }
    Ok(pages)
}

fn handle_live(
    transport: &dyn Transport,
    cache: &Cache,
    account: &mut Option<Account>,
    realtime: &mut Realtime,
    note: LiveNote,
    emit: &impl Fn(Event),
) {
    match note {
        LiveNote::AuthExpired => {
            realtime.reset();
            emit(Event::AuthExpired);
        }
        LiveNote::State(state) => {
            if state == Link::Live {
                let became = !realtime.live;
                realtime.live = true;
                emit(Event::Link {
                    state,
                    updated: false,
                });
                if became {
                    realtime.fresh.clear();
                    if let (Some(account), Some(channel_id)) =
                        (account.as_mut(), realtime.selected.clone())
                        && let Ok(page) = mattermost::reload_posts(
                            transport,
                            cache,
                            &account.site_url,
                            &account.token,
                            &mut account.users,
                            &channel_id,
                        )
                    {
                        realtime.fresh.insert(channel_id);
                        emit(Event::Messages { page });
                    }
                }
            } else {
                if matches!(state, Link::Reconnecting | Link::Offline) {
                    realtime.live = false;
                    realtime.fresh.clear();
                }
                emit(Event::Link {
                    state,
                    updated: false,
                });
            }
        }
        LiveNote::Posted(post) => {
            let Some(account) = account.as_mut() else {
                return;
            };
            let pending = post.pending_post_id.clone();
            let Ok(message) = mattermost::message_from_post(
                transport,
                &account.site_url,
                &account.token,
                &mut account.users,
                &post,
            ) else {
                return;
            };
            if !pending.is_empty() {
                let _ = cache.delete_message(&pending);
                emit(Event::Removed {
                    channel_id: message.channel_id.clone(),
                    post_id: pending,
                });
            }
            remember_user(cache, account, &message.user_id);
            publish(cache, emit, &message);
            let selected = realtime.selected.as_deref() == Some(message.channel_id.as_str());
            if !selected {
                if let Some(room) = account
                    .sidebar
                    .rooms
                    .iter_mut()
                    .find(|room| room.id == message.channel_id)
                {
                    room.unread = true;
                    if mattermost::mentions_me(&message.body_source, &account.me) {
                        room.mentions = room.mentions.saturating_add(1);
                    }
                }
                emit(Event::Sidebar {
                    sidebar: account.sidebar.clone(),
                });
            }
        }
        LiveNote::Edited(post) => {
            let Some(account) = account.as_mut() else {
                return;
            };
            if let Ok(message) = mattermost::message_from_post(
                transport,
                &account.site_url,
                &account.token,
                &mut account.users,
                &post,
            ) {
                remember_user(cache, account, &message.user_id);
                publish(cache, emit, &message);
            }
        }
        LiveNote::Deleted { id, channel_id } => {
            let _ = cache.delete_message(&id);
            emit(Event::Removed {
                channel_id,
                post_id: id,
            });
        }
    }
}

fn remember_user(cache: &Cache, account: &Account, user_id: &str) {
    if let Some(user) = account.users.iter().find(|user| user.id == user_id) {
        let _ = cache.upsert_user(user);
    }
}

fn publish(cache: &Cache, emit: &impl Fn(Event), message: &Message) {
    let _ = cache.upsert_message(message);
    emit(Event::Upsert {
        message: message.clone(),
    });
}

fn optimistic_message(account: &Account, channel_id: &str, text: &str, pending: &str) -> Message {
    let portrait = account
        .me
        .avatar
        .as_ref()
        .map(|bytes| crate::core::Portrait {
            user_id: account.me.id.clone(),
            bytes: bytes.clone(),
        });
    Message {
        id: pending.to_string(),
        channel_id: channel_id.to_string(),
        user_id: account.me.id.clone(),
        author_name: account.me.display_name.clone(),
        body_source: text.to_string(),
        body: crate::emoji::expand_shortcodes(text),
        create_at: mattermost::now_millis(),
        portrait,
    }
}

/// Log in against recorded fixtures, warm the cache for every room, return a
/// snapshot of the account plus the open room's page. Used by demo/snapshot.
pub fn demo_state() -> Result<(Account, MessagePage)> {
    let transport = Arc::new(crate::fixtures::recorded_replay());
    let cache = Cache::open_memory()?;
    let mut account = mattermost::login(
        transport.as_ref(),
        "https://mm.example.test",
        "ada",
        "password",
        None,
    )?;
    mattermost::bootstrap(transport.as_ref(), &mut account)?;
    mattermost::persist_account(&cache, &account, &account.users)?;
    let mut page = None;
    for room in &account.sidebar.rooms {
        let loaded = mattermost::page_messages(
            transport.as_ref(),
            &cache,
            &account.site_url,
            &account.token,
            &mut account.users,
            &room.id,
        )?;
        if room.id == "ch-town" {
            page = Some(loaded);
        }
    }
    Ok((account, page.context("town square page")?))
}
