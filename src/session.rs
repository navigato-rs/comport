//! Background session: cache + HTTP never run on the UI/event-loop thread.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use crate::cache::Cache;
use crate::core::{MessagePage, Sidebar};
use crate::mattermost::{self, Account};
use crate::net::Transport;

pub enum Command {
    Login {
        site: String,
        login_id: String,
        password: String,
        totp: String,
        pat: String,
    },
    LoadHistory {
        channel_id: String,
    },
    SetFavorite {
        channel_id: String,
        favorite: bool,
    },
    Shutdown,
}

pub enum Event {
    Ready { account: Account },
    Sidebar { sidebar: Sidebar },
    Messages { page: MessagePage },
    Error { message: String },
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
        let worker = thread::Builder::new()
            .name("comport-session".into())
            .spawn(move || {
                let _ = ready_tx.send(thread::current().id());
                worker_loop(transport, cache, cmd_rx, ev_tx, wake);
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
        self.cmd
            .send(Command::Login {
                site: site.into(),
                login_id: login_id.into(),
                password: password.into(),
                totp: String::new(),
                pat: String::new(),
            })
            .context("session closed")
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

    pub fn load_history(&self, channel_id: &str) -> Result<()> {
        self.cmd
            .send(Command::LoadHistory {
                channel_id: channel_id.into(),
            })
            .context("session closed")
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
                Event::Error { message } => anyhow::bail!(message),
                _ => {}
            }
        }
    }

    pub fn wait_ready(&self, timeout: Duration) -> Result<Account> {
        loop {
            match self.recv_timeout(timeout)? {
                Event::Ready { account } => return Ok(account),
                Event::Error { message } => anyhow::bail!(message),
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

fn worker_loop(
    transport: Arc<dyn Transport>,
    cache: Cache,
    cmd: Receiver<Command>,
    ev: Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    let emit = |event: Event| {
        let _ = ev.send(event);
        wake();
    };
    let mut account: Option<Account> = None;
    while let Ok(command) = cmd.recv() {
        match command {
            Command::Shutdown => break,
            Command::Login {
                site,
                login_id,
                password,
                totp,
                pat,
            } => {
                match login_and_bootstrap(
                    transport.as_ref(),
                    &cache,
                    &site,
                    &login_id,
                    &password,
                    &totp,
                    &pat,
                ) {
                    Ok(ready) => {
                        account = Some(ready.clone());
                        emit(Event::Ready { account: ready });
                    }
                    Err(error) => {
                        emit(Event::Error {
                            message: format!("{error:#}"),
                        });
                    }
                }
            }
            Command::LoadHistory { channel_id } => {
                let Some(account) = account.as_mut() else {
                    emit(Event::Error {
                        message: "not logged in".into(),
                    });
                    continue;
                };
                match mattermost::page_messages(
                    transport.as_ref(),
                    &cache,
                    &account.site_url,
                    &account.token,
                    &mut account.users,
                    &channel_id,
                ) {
                    Ok(page) => {
                        emit(Event::Messages { page });
                    }
                    Err(error) => {
                        emit(Event::Error {
                            message: format!("{error:#}"),
                        });
                    }
                }
            }
            Command::SetFavorite {
                channel_id,
                favorite,
            } => {
                let Some(account) = account.as_mut() else {
                    emit(Event::Error {
                        message: "not logged in".into(),
                    });
                    continue;
                };
                mattermost::set_favorite(account, &cache, &channel_id, favorite);
                emit(Event::Sidebar {
                    sidebar: account.sidebar.clone(),
                });
            }
        }
    }
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
