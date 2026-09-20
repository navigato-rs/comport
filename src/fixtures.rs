//! Recorded Mattermost REST bodies. Same bytes the tests load from `tests/data/mm`.

use crate::net::{Replay, Response, bytes_response, json_response};

fn data(name: &str) -> &'static str {
    match name {
        "login.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/mm/login.json"
        )),
        "teams.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/mm/teams.json"
        )),
        "channels.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/mm/channels.json"
        )),
        "categories.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/mm/categories.json"
        )),
        "users.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/mm/users.json"
        )),
        "posts_town.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/mm/posts_town.json"
        )),
        "posts_offtopic.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/mm/posts_offtopic.json"
        )),
        "posts_secret.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/mm/posts_secret.json"
        )),
        "posts_dm.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/mm/posts_dm.json"
        )),
        _ => panic!("unknown fixture {name}"),
    }
}

/// Tiny solid-color PNG used as a profile picture.
pub fn portrait_png(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 8, 8);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        let row = [r, g, b].repeat(8);
        let mut image = Vec::with_capacity(8 * 24);
        for _ in 0..8 {
            image.extend_from_slice(&row);
        }
        writer.write_image_data(&image).expect("png data");
    }
    bytes
}

pub fn recorded_replay() -> Replay {
    let mut replay = Replay::new();
    let mut login = json_response(200, data("login.json"));
    login
        .headers
        .push(("Token".into(), "fixture-session-token".into()));
    replay.insert("POST", "/api/v4/users/login", login);
    replay.insert(
        "GET",
        "/api/v4/users/me",
        json_response(200, data("login.json")),
    );
    replay.insert(
        "GET",
        "/api/v4/users/me/teams",
        json_response(200, data("teams.json")),
    );
    replay.insert(
        "GET",
        "/api/v4/users/me/teams/team-acme/channels",
        json_response(200, data("channels.json")),
    );
    replay.insert(
        "GET",
        "/api/v4/users/user-me/teams/team-acme/channels/categories",
        json_response(200, data("categories.json")),
    );
    replay.insert(
        "POST",
        "/api/v4/users/ids",
        json_response(200, data("users.json")),
    );
    replay.insert(
        "GET",
        "/api/v4/channels/ch-town/posts",
        json_response(200, data("posts_town.json")),
    );
    replay.insert(
        "GET",
        "/api/v4/channels/ch-offtopic/posts",
        json_response(200, data("posts_offtopic.json")),
    );
    replay.insert(
        "GET",
        "/api/v4/channels/ch-secret/posts",
        json_response(200, data("posts_secret.json")),
    );
    replay.insert(
        "GET",
        "/api/v4/channels/ch-dm-bob/posts",
        json_response(200, data("posts_dm.json")),
    );
    replay.insert(
        "GET",
        "/api/v4/users/user-me/image",
        png_user(0x1c, 0x58, 0xd9),
    );
    replay.insert(
        "GET",
        "/api/v4/users/user-alice/image",
        png_user(0xe6, 0x4a, 0x19),
    );
    replay.insert(
        "GET",
        "/api/v4/users/user-bob/image",
        png_user(0x3d, 0xa8, 0x63),
    );
    replay
}

fn png_user(r: u8, g: u8, b: u8) -> Response {
    bytes_response(200, "image/png", portrait_png(r, g, b))
}
