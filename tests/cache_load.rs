//! Cache-hit paging: second open of a room does not touch HTTP and runs on the worker.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use comport::cache::Cache;
use comport::mattermost;
use comport::net::{Forbidden, Transport};
use comport::session::Session;

#[test]
fn second_room_open_is_cache_hit_on_worker_without_http() {
    let ui_thread = thread::current().id();
    println!("cache_load ui_thread={ui_thread:?}");

    let replay = Arc::new(comport::fixtures::recorded_replay());
    let cache = Cache::open_memory().expect("cache");
    let session = Session::spawn(replay.clone(), cache);
    session.login("ada", "password").expect("login cmd");
    let account = session.wait_ready(Duration::from_secs(5)).expect("ready");
    assert_eq!(account.me.username, "ada");

    session.load_history("ch-town").expect("first load");
    let first = session
        .wait_page(Duration::from_secs(5))
        .expect("first page");
    assert!(!first.from_cache, "first page should come from fixtures");
    assert_ne!(
        first.loaded_on, ui_thread,
        "page_messages must run on the session worker, not the test/UI thread"
    );
    println!(
        "cache_load first from_cache={} loaded_on={:?} http={}",
        first.from_cache,
        first.loaded_on,
        replay.call_count()
    );
    let http_after_first = replay.call_count();
    assert!(http_after_first > 0);

    session.load_history("ch-town").expect("second load");
    let second = session
        .wait_page(Duration::from_secs(5))
        .expect("second page");
    println!(
        "cache_load second from_cache={} loaded_on={:?} http={}",
        second.from_cache,
        second.loaded_on,
        replay.call_count()
    );
    assert!(second.from_cache, "second open must be a cache hit");
    assert_eq!(
        replay.call_count(),
        http_after_first,
        "cache hit must not perform HTTP"
    );
    assert_eq!(second.loaded_on, session.worker_thread());
    assert_ne!(second.loaded_on, ui_thread);
    assert_eq!(second.messages.len(), first.messages.len());
    assert!(
        second
            .messages
            .iter()
            .any(|message| message.body.contains('😄')),
        "cached page still has expanded emoji"
    );
}

#[test]
fn cache_page_with_forbidden_transport() {
    let replay = Arc::new(comport::fixtures::recorded_replay());
    let cache = Cache::open_memory().expect("cache");
    let mut account = mattermost::login(replay.as_ref(), "ada", "password").expect("login");
    mattermost::bootstrap(replay.as_ref(), &mut account).expect("bootstrap");
    mattermost::persist_account(&cache, &account, &account.users).expect("persist");
    mattermost::page_messages(
        replay.as_ref() as &dyn Transport,
        &cache,
        &account.token,
        &mut account.users,
        "ch-town",
    )
    .expect("warm cache");

    let forbidden = Forbidden;
    let page = mattermost::page_messages(
        &forbidden,
        &cache,
        &account.token,
        &mut account.users,
        "ch-town",
    )
    .expect("cache hit must not call Forbidden");
    println!(
        "cache_load forbidden_hit from_cache={} messages={}",
        page.from_cache,
        page.messages.len()
    );
    assert!(page.from_cache);
    assert!(!page.messages.is_empty());
}
