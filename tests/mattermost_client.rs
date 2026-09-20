//! Drive the shipped login, sidebar, favorite, and message-page functions
//! against recorded Mattermost JSON (no live server).

use std::sync::Arc;

use comport::cache::Cache;
use comport::core::SidebarSection;
use comport::emoji::expand_shortcodes;
use comport::fixtures::recorded_replay;
use comport::mattermost;
use comport::net::Transport;
use comport::ui::sidebar_has_left_list;

fn fixture_login() -> (Arc<comport::net::Replay>, Cache, mattermost::Account) {
    let replay = Arc::new(recorded_replay());
    let cache = Cache::open_memory().expect("cache");
    let mut account = mattermost::login(
        replay.as_ref(),
        "https://mm.example.test",
        "ada",
        "password",
        None,
    )
    .expect("login");
    mattermost::bootstrap(replay.as_ref(), &mut account).expect("bootstrap");
    mattermost::persist_account(&cache, &account, &account.users).expect("persist");
    (replay, cache, account)
}

#[test]
fn login_lists_rooms_on_the_left_with_favorites() {
    let (_replay, _cache, account) = fixture_login();
    assert!(sidebar_has_left_list(&account.sidebar));
    let grouped = account.sidebar.grouped();
    let favorites = grouped
        .iter()
        .find(|(section, _)| *section == SidebarSection::Favorites)
        .map(|(_, rooms)| {
            rooms
                .iter()
                .map(|room| room.id.as_str())
                .collect::<Vec<_>>()
        })
        .expect("favorites section");
    assert_eq!(favorites, vec!["ch-offtopic"]);
    let channels = grouped
        .iter()
        .find(|(section, _)| *section == SidebarSection::Channels)
        .map(|(_, rooms)| {
            rooms
                .iter()
                .map(|room| room.id.as_str())
                .collect::<Vec<_>>()
        })
        .expect("channels section");
    assert!(channels.contains(&"ch-town"));
    assert!(channels.contains(&"ch-secret"));
    assert!(!channels.contains(&"ch-offtopic"));
    let dms = grouped
        .iter()
        .find(|(section, _)| *section == SidebarSection::DirectMessages)
        .map(|(_, rooms)| {
            rooms
                .iter()
                .map(|r| r.display_name.as_str())
                .collect::<Vec<_>>()
        })
        .expect("dm section");
    assert_eq!(dms, vec!["Bob Katz"]);
}

#[test]
fn starring_moves_a_room_into_favorites_and_back() {
    let (_replay, cache, mut account) = fixture_login();
    assert!(mattermost::set_favorite(
        &mut account,
        &cache,
        "ch-offtopic",
        false
    ));
    let grouped = account.sidebar.grouped();
    assert!(
        grouped
            .iter()
            .all(|(section, _)| *section != SidebarSection::Favorites)
    );
    assert!(mattermost::set_favorite(
        &mut account,
        &cache,
        "ch-town",
        true
    ));
    let favorites: Vec<&str> = account
        .sidebar
        .grouped()
        .into_iter()
        .find(|(section, _)| *section == SidebarSection::Favorites)
        .unwrap()
        .1
        .into_iter()
        .map(|room| room.id.as_str())
        .collect();
    assert_eq!(favorites, vec!["ch-town"]);
    let persisted = cache.load_sidebar().expect("reload sidebar");
    assert!(persisted.room("ch-town").expect("town").favorite);
    assert!(!persisted.room("ch-offtopic").expect("offtopic").favorite);
}

#[test]
fn message_page_has_portrait_and_expanded_emoji() {
    let (replay, cache, mut account) = fixture_login();
    let page = mattermost::page_messages(
        replay.as_ref() as &dyn Transport,
        &cache,
        &account.site_url,
        &account.token,
        &mut account.users,
        "ch-town",
    )
    .expect("page town");
    assert!(!page.from_cache);
    let hello = page
        .messages
        .iter()
        .find(|message| message.id == "post-town-1")
        .expect("alice hello");
    assert_eq!(hello.body_source, "Hello team :smile:");
    assert_eq!(hello.body, expand_shortcodes("Hello team :smile:"));
    assert!(
        hello.body.contains('😄'),
        "shortcode must expand to a glyph, got {:?}",
        hello.body
    );
    assert!(
        !hello.body.contains(":smile:"),
        "expanded body still contains :smile: {:?}",
        hello.body
    );
    let portrait = hello.portrait.as_ref().expect("author portrait");
    assert_eq!(portrait.user_id, "user-alice");
    assert!(
        portrait.bytes.starts_with(b"\x89PNG"),
        "portrait must be PNG bytes, got {} bytes",
        portrait.bytes.len()
    );
}

#[test]
fn normalize_site_requires_https() {
    assert_eq!(
        mattermost::normalize_site(" chat.company.com/ ").unwrap(),
        "https://chat.company.com"
    );
    assert_eq!(
        mattermost::normalize_site("https://mm.example.com/sub").unwrap(),
        "https://mm.example.com/sub"
    );
    assert!(mattermost::normalize_site("http://mm.example.com").is_err());
    assert!(mattermost::normalize_site("https://user:pass@host").is_err());
}

#[test]
fn personal_access_token_skips_password_login() {
    let replay = Arc::new(recorded_replay());
    let mut account = mattermost::login_with_token(
        replay.as_ref(),
        "https://mm.example.test",
        "fixture-session-token",
    )
    .expect("PAT login");
    mattermost::bootstrap(replay.as_ref(), &mut account).expect("bootstrap");
    assert_eq!(account.me.username, "ada");
    let calls = replay.calls();
    assert!(
        !calls
            .iter()
            .any(|(method, path)| method == "POST" && path == "/api/v4/users/login"),
        "PAT must not POST /users/login, got {calls:?}"
    );
}
