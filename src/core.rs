//! Protocol-agnostic chat models used by the Mattermost backend and the UI.

use std::thread::ThreadId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoomKind {
    Public,
    Private,
    Direct,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub kind: RoomKind,
    pub favorite: bool,
    /// Mention count from channel membership. Not a poll; websocket bumps it.
    pub mentions: u32,
    /// A post arrived in this room while it was not the open one.
    pub unread: bool,
}

/// Realtime link. The UI paints this; nothing animates it on a timer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Link {
    Offline,
    Connecting,
    Live,
    Reconnecting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum SidebarSection {
    Favorites,
    Channels,
    DirectMessages,
}

impl SidebarSection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Favorites => "FAVORITES",
            Self::Channels => "CHANNELS",
            Self::DirectMessages => "DIRECT MESSAGES",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Sidebar {
    pub rooms: Vec<Room>,
}

impl Sidebar {
    pub fn room(&self, id: &str) -> Option<&Room> {
        self.rooms.iter().find(|room| room.id == id)
    }

    /// Mattermost sidebar: starred rooms live only under Favorites.
    pub fn grouped(&self) -> Vec<(SidebarSection, Vec<&Room>)> {
        let mut favorites = Vec::new();
        let mut channels = Vec::new();
        let mut dms = Vec::new();
        for room in &self.rooms {
            if room.favorite {
                favorites.push(room);
                continue;
            }
            match room.kind {
                RoomKind::Direct => dms.push(room),
                RoomKind::Public | RoomKind::Private => channels.push(room),
            }
        }
        let mut out = Vec::new();
        if !favorites.is_empty() {
            out.push((SidebarSection::Favorites, favorites));
        }
        if !channels.is_empty() {
            out.push((SidebarSection::Channels, channels));
        }
        if !dms.is_empty() {
            out.push((SidebarSection::DirectMessages, dms));
        }
        out
    }

    /// Star or unstar a room. Returns whether membership changed.
    pub fn set_favorite(&mut self, channel_id: &str, favorite: bool) -> bool {
        let Some(room) = self.rooms.iter_mut().find(|room| room.id == channel_id) else {
            return false;
        };
        if room.favorite == favorite {
            return false;
        }
        room.favorite = favorite;
        true
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub avatar: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Portrait {
    pub user_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub user_id: String,
    pub author_name: String,
    pub body_source: String,
    pub body: String,
    pub create_at: i64,
    pub portrait: Option<Portrait>,
}

#[derive(Clone, Debug)]
pub struct MessagePage {
    pub channel_id: String,
    pub messages: Vec<Message>,
    pub from_cache: bool,
    pub loaded_on: ThreadId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub display_name: String,
}
