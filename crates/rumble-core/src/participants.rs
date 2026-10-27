// SPDX-License-Identifier: EPL-2.0

//! The RoboWiki participants lists (spec §16.1): their URLs, parsing a page, the archive
//! mirror, and checking that a jar is the entry it was fetched for. Shared by the seeding
//! importer (`rumble-client`) and the daily sync of the `batch` Lambda (§16.5), so both read
//! a page exactly alike.

use crate::config::Config;
use crate::ids::Division;
use crate::jar::{JarInfo, validate_named};
use serde::Serialize;
use std::collections::HashSet;

/// RoboWiki's 1v1 participants page, raw.
pub const LIST_1V1: &str =
    "https://robowiki.net/w/index.php?title=RoboRumble/Participants&action=raw";
/// RoboWiki's melee participants page, raw.
pub const LIST_MELEE: &str =
    "https://robowiki.net/w/index.php?title=RoboRumble/Participants/Melee&action=raw";
/// The archive the classic rumble clients fall back on.
pub const DEFAULT_MIRROR: &str =
    "https://robocode-archive.strangeautomata.com/robots/{id}_{version}.jar";

/// Download limit per jar (§16.1).
pub const MAX_JAR_BYTES: u64 = 5 << 20;
/// The page itself.
pub const MAX_LIST_BYTES: u64 = 16 << 20;

/// The list URL of a division.
pub fn default_list(division: Division) -> &'static str {
    match division {
        Division::OneVsOne => LIST_1V1,
        Division::Melee => LIST_MELEE,
    }
}

/// One line of the list: `<id> <version>,<url>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub id: String,
    pub version: String,
    pub url: String,
    /// 1-based line in the page.
    pub line: usize,
}

/// A line that is not an entry, or a repeated one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub line: usize,
    pub text: String,
}

/// What a participants page holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ParsedList {
    pub entries: Vec<Entry>,
    /// Lines repeating an earlier `id version`; the first one counts.
    pub duplicates: Vec<Problem>,
    pub malformed: Vec<Problem>,
}

/// Parses a page in `action=raw` form. Entries are the lines between `<pre>` and `</pre>`
/// (all lines if there is no `<pre>`, for plain files). Each is `<id> <version>,<url>`: the id
/// ends at the first space, the version at the first comma after it (versions may hold
/// spaces, never commas; the URL may hold anything). Blank and `#` lines are skipped.
pub fn parse_list(text: &str) -> ParsedList {
    let mut out = ParsedList::default();
    let mut seen = HashSet::new();
    let mut inside = !text.contains("<pre>");
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let mut raw = raw;
        if !inside {
            match raw.find("<pre>") {
                Some(at) => {
                    inside = true;
                    raw = &raw[at + 5..];
                }
                None => continue,
            }
        }
        let mut done = false;
        if let Some(at) = raw.find("</pre>") {
            raw = &raw[..at];
            done = true;
        }
        let trimmed = raw.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            match parse_entry(trimmed, line) {
                Some(entry) => {
                    if seen.insert((entry.id.clone(), entry.version.clone())) {
                        out.entries.push(entry);
                    } else {
                        out.duplicates.push(Problem {
                            line,
                            text: trimmed.to_string(),
                        });
                    }
                }
                None => out.malformed.push(Problem {
                    line,
                    text: trimmed.to_string(),
                }),
            }
        }
        if done {
            break;
        }
    }
    out
}

fn parse_entry(text: &str, line: usize) -> Option<Entry> {
    let (id, rest) = text.split_once(' ')?;
    let (version, url) = rest.split_once(',')?;
    let (version, url) = (version.trim(), url.trim());
    if id.is_empty() || version.is_empty() || url.is_empty() || id.contains(',') {
        return None;
    }
    Some(Entry {
        id: id.to_string(),
        version: version.to_string(),
        url: url.to_string(),
        line,
    })
}

/// The `{id}_{version}.jar` URL of the mirror template (version percent-encoded).
pub fn mirror_url(template: &str, id: &str, version: &str) -> String {
    template
        .replace("{id}", id)
        .replace("{version}", &percent_encode(version))
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The jar must be valid and say it is this entry. A missing `robot.version` takes the
/// list's (classic's list is authoritative; the second value says so); a different one is a
/// mismatch. Errors read `invalid (...)` or `mismatch (...)`.
pub fn check_entry(
    bytes: &[u8],
    id: &str,
    version: &str,
    config: &Config,
) -> Result<(JarInfo, bool), String> {
    let (info, from_list) = validate_named(bytes, config, Some(version), id)
        .map_err(|messages| format!("invalid ({})", messages.join("; ")))?;
    if info.bot_id != id || info.version != version {
        return Err(format!(
            "mismatch (properties say {} {})",
            info.bot_id, info.version
        ));
    }
    Ok((info, from_list))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_template() {
        assert_eq!(
            mirror_url(DEFAULT_MIRROR, "a.B", "1.0 beta+2"),
            "https://robocode-archive.strangeautomata.com/robots/a.B_1.0%20beta%2B2.jar"
        );
    }

    #[test]
    fn entries_and_problems() {
        let list = parse_list("x\n<pre>\na.B 1.0,http://x\n# c\na.B 1.0,http://y\nbad\n</pre>");
        assert_eq!(list.entries.len(), 1);
        assert_eq!((list.duplicates[0].line, list.malformed[0].line), (5, 6));
    }
}
