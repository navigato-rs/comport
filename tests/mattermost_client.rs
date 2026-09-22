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
fn password_rejection_points_at_browser_sso() {
    use comport::net::{Replay, json_response};

    let mut replay = Replay::new();
    replay.insert(
        "POST",
        "/api/v4/users/login",
        json_response(
            401,
            r#"{"id":"api.user.check_user_password.invalid.app_error","message":"no"}"#,
        ),
    );
    replay.insert(
        "GET",
        "/api/v4/config/client",
        json_response(
            200,
            r#"{"EnableSignInWithEmail":"false","EnableSaml":"true","EnableSignUpWithOffice365":"true"}"#,
        ),
    );
    let error = mattermost::login(&replay, "https://mm.example.test", "ada", "nope", None)
        .expect_err("password must fail");
    let message = format!("{error:#}");
    assert!(
        message.contains("single sign-on"),
        "SSO servers should not send the user hunting for a token: {message}"
    );
    assert!(message.contains("SAML"), "{message}");
    assert!(message.contains("Entra ID"), "{message}");
}

#[test]
fn desktop_token_login_is_one_post() {
    use comport::net::{Replay, json_response};

    let mut replay = Replay::new();
    let mut login = json_response(200, include_str!("data/mm/login.json"));
    login.headers.push(("Token".into(), "from-desktop".into()));
    replay.insert("POST", "/api/v4/users/login/desktop_token", login);
    let account = mattermost::login_with_desktop_token(
        &replay,
        "https://mm.example.test",
        "servertokenvalue123456",
    )
    .expect("desktop token");
    assert_eq!(account.token, "from-desktop");
    assert_eq!(account.me.username, "ada");
    assert_eq!(replay.call_count(), 1, "do not poll the desktop token");
}

#[test]
fn websocket_frames_cover_auth_and_posts() {
    assert!(matches!(
        mattermost::parse_ws_message(r#"{"status":"OK","seq_reply":1}"#),
        Some(mattermost::Incoming::AuthOk)
    ));
    assert!(matches!(
        mattermost::parse_ws_message(r#"{"status":"FAIL","seq_reply":1}"#),
        Some(mattermost::Incoming::AuthFail)
    ));
    let text = r#"{"event":"posted","data":{"sender_name":"Ada","post":"{\"id\":\"p1\",\"message\":\"Hi :smile:\",\"channel_id\":\"c1\",\"user_id\":\"u1\",\"create_at\":5,\"pending_post_id\":\"local-1\"}"},"seq":2}"#;
    match mattermost::parse_ws_message(text).expect("posted") {
        mattermost::Incoming::Posted(post) => {
            assert_eq!(post.id, "p1");
            assert_eq!(post.channel_id, "c1");
            assert_eq!(post.message, "Hi :smile:");
            assert_eq!(post.pending_post_id, "local-1");
            assert_eq!(post.sender_name, "Ada");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(mattermost::parse_ws_message(r#"{"event":"typing","data":{}}"#).is_none());
    assert_eq!(
        mattermost::websocket_url("https://chat.company.com/team", ""),
        "wss://chat.company.com/team/api/v4/websocket"
    );
    assert_eq!(
        mattermost::websocket_url("https://chat.company.com", "wss://chat.company.com"),
        "wss://chat.company.com/api/v4/websocket"
    );
}

#[test]
fn create_post_uses_the_server_post() {
    use comport::net::{Replay, json_response};

    let mut replay = Replay::new();
    replay.insert(
        "POST",
        "/api/v4/posts",
        json_response(
            201,
            r#"{"id":"p9","message":"Hi :wave:","channel_id":"c","user_id":"user-me","create_at":9}"#,
        ),
    );
    let mut users = Vec::new();
    let message = mattermost::create_post(
        &replay,
        "https://mm.example.test",
        "token",
        &mut users,
        "c",
        "Hi :wave:",
        "local-abc",
    )
    .expect("create");
    assert_eq!(message.id, "p9");
    assert_eq!(message.body_source, "Hi :wave:");
    assert_eq!(message.body, expand_shortcodes("Hi :wave:"));
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
