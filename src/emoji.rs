//! Mattermost `:shortcode:` expansion for the common Unicode set.

const TABLE: &[(&str, &str)] = &[
    ("+1", "👍"),
    ("100", "💯"),
    ("blush", "😊"),
    ("clap", "👏"),
    ("confused", "😕"),
    ("cry", "😢"),
    ("eyes", "👀"),
    ("fire", "🔥"),
    ("grinning", "😀"),
    ("heart", "❤️"),
    ("joy", "😂"),
    ("laughing", "😆"),
    ("ok_hand", "👌"),
    ("rocket", "🚀"),
    ("smile", "😄"),
    ("smiley", "😃"),
    ("star", "⭐"),
    ("tada", "🎉"),
    ("thinking", "🤔"),
    ("thumbsdown", "👎"),
    ("thumbsup", "👍"),
    ("wave", "👋"),
    ("wink", "😉"),
    ("x", "❌"),
];

fn lookup(name: &str) -> Option<&'static str> {
    TABLE
        .binary_search_by_key(&name, |entry| entry.0)
        .ok()
        .map(|i| TABLE[i].1)
}

/// Replace `:shortcode:` tokens with Unicode emoji. Unknown tokens stay as-is.
pub fn expand_shortcodes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find(':') {
            Some(end) if end > 0 && end <= 32 && is_shortcode_name(&after[..end]) => {
                let name = &after[..end];
                if let Some(glyph) = lookup(name) {
                    out.push_str(glyph);
                    rest = &after[end + 1..];
                    continue;
                }
                out.push(':');
                rest = after;
            }
            _ => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_shortcode_name(name: &str) -> bool {
    name.bytes().all(|b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'+' || b == b'-'
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn table_is_sorted() {
        let names: Vec<&str> = super::TABLE.iter().map(|e| e.0).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted);
    }
}
