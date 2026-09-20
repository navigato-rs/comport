//! SQLite message/channel cache. Owned by the session worker, never the UI thread.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::core::{Message, Portrait, Room, RoomKind, Sidebar, User};

pub struct Cache {
    conn: Connection,
}

impl Cache {
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("open in-memory SQLite")?;
        let cache = Self { conn };
        cache.init()?;
        Ok(cache)
    }

    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("open SQLite {path}"))?;
        let cache = Self { conn };
        cache.init()?;
        Ok(cache)
    }

    fn init(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE IF NOT EXISTS rooms (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    favorite INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS users (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    avatar BLOB
                );
                CREATE TABLE IF NOT EXISTS messages (
                    id TEXT PRIMARY KEY,
                    channel_id TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    body_source TEXT NOT NULL,
                    body TEXT NOT NULL,
                    create_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS messages_channel ON messages(channel_id, create_at);
                ",
            )
            .context("init cache schema")?;
        Ok(())
    }

    pub fn replace_sidebar(&self, sidebar: &Sidebar) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM rooms", [])?;
        for room in &sidebar.rooms {
            tx.execute(
                "INSERT INTO rooms (id, name, display_name, kind, favorite) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    room.id,
                    room.name,
                    room.display_name,
                    kind_str(room.kind),
                    room.favorite as i64
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_sidebar(&self) -> Result<Sidebar> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, display_name, kind, favorite FROM rooms ORDER BY display_name",
        )?;
        let rooms = stmt
            .query_map([], |row| {
                Ok(Room {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    display_name: row.get(2)?,
                    kind: kind_from_str(&row.get::<_, String>(3)?),
                    favorite: row.get::<_, i64>(4)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Sidebar { rooms })
    }

    pub fn set_favorite(&self, channel_id: &str, favorite: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE rooms SET favorite = ?1 WHERE id = ?2",
            params![favorite as i64, channel_id],
        )?;
        Ok(())
    }

    pub fn upsert_user(&self, user: &User) -> Result<()> {
        self.conn.execute(
            "INSERT INTO users (id, username, display_name, avatar) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                username = excluded.username,
                display_name = excluded.display_name,
                avatar = COALESCE(excluded.avatar, users.avatar)",
            params![user.id, user.username, user.display_name, user.avatar],
        )?;
        Ok(())
    }

    pub fn user(&self, id: &str) -> Result<Option<User>> {
        self.conn
            .query_row(
                "SELECT id, username, display_name, avatar FROM users WHERE id = ?1",
                [id],
                |row| {
                    Ok(User {
                        id: row.get(0)?,
                        username: row.get(1)?,
                        display_name: row.get(2)?,
                        avatar: row.get(3)?,
                    })
                },
            )
            .optional()
            .context("load user")
    }

    pub fn replace_messages(&self, channel_id: &str, messages: &[Message]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM messages WHERE channel_id = ?1", [channel_id])?;
        for message in messages {
            tx.execute(
                "INSERT INTO messages (id, channel_id, user_id, body_source, body, create_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    message.id,
                    message.channel_id,
                    message.user_id,
                    message.body_source,
                    message.body,
                    message.create_at
                ],
            )?;
            if let Some(portrait) = &message.portrait {
                tx.execute(
                    "UPDATE users SET avatar = ?1 WHERE id = ?2",
                    params![portrait.bytes, portrait.user_id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Page messages for a room from SQLite only. No HTTP.
    pub fn page(&self, channel_id: &str) -> Result<Option<Vec<Message>>> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE channel_id = ?1",
            [channel_id],
            |row| row.get(0),
        )?;
        if count == 0 {
            return Ok(None);
        }
        let mut stmt = self.conn.prepare(
            "SELECT id, channel_id, user_id, body_source, body, create_at
             FROM messages WHERE channel_id = ?1 ORDER BY create_at ASC",
        )?;
        let rows = stmt.query_map([channel_id], |row| {
            Ok(Message {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                user_id: row.get(2)?,
                author_name: String::new(),
                body_source: row.get(3)?,
                body: row.get(4)?,
                create_at: row.get(5)?,
                portrait: None,
            })
        })?;
        let mut messages = Vec::new();
        for row in rows {
            let mut message = row?;
            if let Some(user) = self.user(&message.user_id)? {
                message.author_name = user.display_name;
                if let Some(bytes) = user.avatar {
                    message.portrait = Some(Portrait {
                        user_id: user.id,
                        bytes,
                    });
                }
            }
            messages.push(message);
        }
        Ok(Some(messages))
    }
}

fn kind_str(kind: RoomKind) -> &'static str {
    match kind {
        RoomKind::Public => "O",
        RoomKind::Private => "P",
        RoomKind::Direct => "D",
    }
}

fn kind_from_str(kind: &str) -> RoomKind {
    match kind {
        "P" => RoomKind::Private,
        "D" => RoomKind::Direct,
        _ => RoomKind::Public,
    }
}
