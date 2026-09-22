//! Native Mattermost client. The desktop binary lives in `src/main.rs`.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REVISION: Option<&str> = option_env!("GITHUB_SHA");

pub mod cache;
pub mod core;
pub mod emoji;
pub mod fixtures;
pub mod handoff;
pub mod https;
pub mod mattermost;
pub mod net;
mod provider;
pub mod session;
pub mod settings;
pub mod snapshot;
pub mod ui;
pub mod window;
mod ws;

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_semver() {
        let parts: Vec<&str> = crate::VERSION.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())));
    }
}
