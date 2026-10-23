// SPDX-License-Identifier: EPL-2.0

//! Static validation of a submitted robot jar (spec §12.1), on the bytes alone.

use crate::codesize::{self, MAX_CLASS_MAJOR};
use crate::config::Config;
use crate::ids::{WeightClass, validate_bot_id, validate_version};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{self, Cursor, Read};

/// How deep jars inside jars are followed (classic's codesize tool follows them without
/// limit).
const MAX_NESTING: u32 = 3;

/// What validation learned about a jar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JarInfo {
    /// The fully qualified class name without version.
    pub bot_id: String,
    pub version: String,
    /// `robot.classname`, the same as the bot id.
    pub main_class: String,
    pub author: String,
    pub description: String,
    pub webpage: String,
    pub codesize: u32,
    pub weight_class: WeightClass,
    /// Lowercase hex SHA-256 of the jar bytes.
    pub sha256: String,
    /// Size of the jar in bytes.
    pub size: u64,
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Validates a jar in the order of §12.1. Returns the metadata, or the messages for the
/// author (all messages of the first stage that fails).
pub fn validate(bytes: &[u8], config: &Config) -> Result<JarInfo, Vec<String>> {
    let limits = &config.jar;

    // 1. Size, zip structure, names, unpacked size.
    if bytes.len() as u64 > limits.max_bytes {
        return Err(vec![format!(
            "the jar is {} bytes; the limit is {} bytes",
            bytes.len(),
            limits.max_bytes
        )]);
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| vec![format!("not a valid jar (zip) file: {e}")])?;
    if archive.len() > limits.max_entries as usize {
        return Err(vec![format!(
            "the jar has {} entries; the limit is {}",
            archive.len(),
            limits.max_entries
        )]);
    }
    let mut budget = Budget {
        unpacked: 0,
        max_unpacked: limits.max_unpacked,
        entries: archive.len() as u64,
        max_entries: u64::from(limits.max_entries),
    };
    let mut problems = Vec::new();
    let mut properties: Vec<(String, Vec<u8>)> = Vec::new();
    let mut classes: Vec<(String, Vec<u8>)> = Vec::new();
    let mut jars: Vec<(String, Vec<u8>)> = Vec::new();
    let mut teams = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| vec![format!("cannot read entry {index} of the jar: {e}")])?;
        let name = entry.name().to_string();
        if let Some(why) = bad_name(&name) {
            problems.push(format!("entry {name:?}: {why}"));
            continue;
        }
        if entry.is_dir() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        let keep = lower.ends_with(".properties")
            || lower.ends_with(".class")
            || lower.ends_with(".jar")
            || lower.ends_with(".team");
        let data = budget
            .read(&mut entry, keep)
            .map_err(|e| vec![format!("entry {name:?}: {e}")])?;
        if lower.ends_with(".team") {
            teams.push(name);
        } else if lower.ends_with(".properties") {
            properties.push((name, data));
        } else if lower.ends_with(".class") {
            classes.push((name, data));
        } else if lower.ends_with(".jar") {
            jars.push((name, data));
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }

    // 2. Properties, version, teams.
    if !teams.is_empty() {
        return Err(vec![format!(
            "team files ({}) are not accepted: teams are not open yet",
            teams.join(", ")
        )]);
    }
    let mut robots: Vec<(String, BTreeMap<String, String>)> = properties
        .into_iter()
        .map(|(name, data)| (name, parse_properties(&decode_latin1(&data))))
        .filter(|(_, props)| props.contains_key("robot.classname"))
        .collect();
    let (properties_name, props) = match robots.len() {
        0 => {
            return Err(vec![
                "no robot properties file: the jar needs one .properties file with robot.classname"
                    .to_string(),
            ]);
        }
        1 => robots.remove(0),
        _ => {
            let names: Vec<_> = robots.iter().map(|(name, _)| name.as_str()).collect();
            return Err(vec![format!(
                "more than one robot properties file ({}): a jar holds exactly one robot",
                names.join(", ")
            )]);
        }
    };
    let get = |key: &str| props.get(key).map(|v| v.trim().to_string());
    let mut problems = Vec::new();
    let version = get("robot.version").unwrap_or_default();
    if version.is_empty() {
        problems.push(format!("{properties_name}: robot.version is missing"));
    } else if let Err(why) = validate_version(&version) {
        problems.push(format!("robot.version: {why}"));
    }
    let classname = get("robot.classname").unwrap_or_default();
    if let Err(why) = validate_bot_id(&classname) {
        problems.push(format!("robot.classname {classname:?}: {why}"));
    }
    if !problems.is_empty() {
        return Err(problems);
    }

    // 3. The main class is in the jar and parses.
    let path = format!("{}.class", classname.replace('.', "/"));
    let Some((_, main)) = classes.iter().find(|(name, _)| *name == path) else {
        return Err(vec![format!(
            "the class {classname} is not in the jar (expected the entry {path})"
        )]);
    };
    let mut problems = Vec::new();
    match codesize::parse_class(main) {
        Ok(info) if info.major > MAX_CLASS_MAJOR => problems.push(format!(
            "{path} is compiled for class file version {} (Java {}); the newest supported is {MAX_CLASS_MAJOR} (Java 21)",
            info.major,
            info.major.saturating_sub(44)
        )),
        Ok(_) => {}
        Err(why) => problems.push(format!("{path}: {why}")),
    }
    if !problems.is_empty() {
        return Err(problems);
    }

    // 4. Codesize, as classic's tool: every .class entry, jars inside jars too.
    let mut total: u64 = 0;
    let mut problems = Vec::new();
    for (name, data) in &classes {
        match codesize::parse_class(data) {
            Ok(info) if info.major > MAX_CLASS_MAJOR => problems.push(format!(
                "{name} is compiled for class file version {} (Java {}); the newest supported is {MAX_CLASS_MAJOR} (Java 21)",
                info.major,
                info.major.saturating_sub(44)
            )),
            Ok(info) => total += u64::from(info.code_size),
            Err(why) => problems.push(format!("{name}: {why}")),
        }
    }
    for (name, data) in &jars {
        match nested_codesize(data, 1, &mut budget) {
            Ok(size) => total += size,
            Err(why) => problems.push(format!("{name}: {why}")),
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let codesize = u32::try_from(total).unwrap_or(u32::MAX);
    if codesize == 0 {
        return Err(vec!["the jar contains no code".to_string()]);
    }

    // 5. Metadata.
    let author = get("robot.author.name")
        .filter(|a| !a.is_empty())
        .or_else(|| {
            classname
                .split_once('.')
                .map(|(first, _)| first.to_string())
        })
        .unwrap_or_default();
    Ok(JarInfo {
        bot_id: classname.clone(),
        version,
        main_class: classname,
        author,
        description: get("robot.description").unwrap_or_default(),
        webpage: get("robot.webpage").unwrap_or_default(),
        codesize,
        weight_class: WeightClass::of(codesize, &config.weight_limits),
        sha256: sha256_hex(bytes),
        size: bytes.len() as u64,
    })
}

/// The codesize of a jar, nothing else checked (the sum of `validate`'s step 4).
pub fn jar_codesize(bytes: &[u8], config: &Config) -> Result<u32, String> {
    let mut budget = Budget {
        unpacked: 0,
        max_unpacked: config.jar.max_unpacked,
        entries: 0,
        max_entries: u64::from(config.jar.max_entries),
    };
    nested_codesize(bytes, 0, &mut budget).map(|total| u32::try_from(total).unwrap_or(u32::MAX))
}

/// Unpacked bytes and entries so far, over the jar and the jars inside it.
struct Budget {
    unpacked: u64,
    max_unpacked: u64,
    entries: u64,
    max_entries: u64,
}

impl Budget {
    /// Reads an entry, counting the bytes actually produced (the headers are not trusted).
    /// The bytes are kept only if `keep`.
    fn read(&mut self, entry: &mut impl Read, keep: bool) -> Result<Vec<u8>, String> {
        let room = self.max_unpacked - self.unpacked;
        let mut limited = entry.take(room + 1);
        let mut data = Vec::new();
        let count = if keep {
            limited.read_to_end(&mut data).map(|n| n as u64)
        } else {
            io::copy(&mut limited, &mut io::sink())
        }
        .map_err(|e| format!("cannot be unpacked: {e}"))?;
        if count > room {
            return Err(format!(
                "the jar unpacks to more than {} bytes",
                self.max_unpacked
            ));
        }
        self.unpacked += count;
        Ok(data)
    }
}

/// The codesize of a jar's `.class` entries and, recursively, of the jars inside it.
fn nested_codesize(bytes: &[u8], depth: u32, budget: &mut Budget) -> Result<u64, String> {
    if depth > MAX_NESTING {
        return Err("jars are nested too deeply".to_string());
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("not a valid jar (zip) file: {e}"))?;
    if depth > 0 {
        budget.entries += archive.len() as u64;
        if budget.entries > budget.max_entries {
            return Err(format!("more than {} entries in all", budget.max_entries));
        }
    }
    let mut total = 0;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("cannot read entry {index}: {e}"))?;
        let name = entry.name().to_string();
        let lower = name.to_ascii_lowercase();
        if entry.is_dir() || !(lower.ends_with(".class") || lower.ends_with(".jar")) {
            continue;
        }
        let data = budget
            .read(&mut entry, true)
            .map_err(|e| format!("{name}: {e}"))?;
        if lower.ends_with(".class") {
            total += u64::from(
                codesize::parse_class(&data)
                    .map_err(|e| format!("{name}: {e}"))?
                    .code_size,
            );
        } else {
            total +=
                nested_codesize(&data, depth + 1, budget).map_err(|e| format!("{name}: {e}"))?;
        }
    }
    Ok(total)
}

/// Why an entry name is refused, if it is.
fn bad_name(name: &str) -> Option<&'static str> {
    if name.contains('\\') {
        Some("contains a backslash")
    } else if name.starts_with('/') || name.as_bytes().get(1) == Some(&b':') {
        Some("is an absolute path")
    } else if name.split('/').any(|part| part == "..") {
        Some("contains \"..\"")
    } else if name.contains('\0') {
        Some("contains a NUL character")
    } else {
        None
    }
}

/// Java `.properties` files are ISO-8859-1.
pub fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// Parses the subset of the Java properties format that robot properties use: `key=value`,
/// `key: value` and `key value`; comment lines starting with `#` or `!`; `\` line
/// continuations; the escapes `\t \n \r \f`, `\uXXXX` and `\c` for any other `c`. Later keys
/// replace earlier ones.
pub fn parse_properties(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in logical_lines(text) {
        let chars: Vec<char> = line.chars().collect();
        // The key ends at the first unescaped `=`, `:` or whitespace.
        let mut end = 0;
        while end < chars.len() {
            match chars[end] {
                '\\' => end += 2,
                '=' | ':' | ' ' | '\t' | '\x0c' => break,
                _ => end += 1,
            }
        }
        let end = end.min(chars.len());
        let mut rest = end;
        while rest < chars.len() && matches!(chars[rest], ' ' | '\t' | '\x0c') {
            rest += 1;
        }
        if rest < chars.len() && matches!(chars[rest], '=' | ':') {
            rest += 1;
            while rest < chars.len() && matches!(chars[rest], ' ' | '\t' | '\x0c') {
                rest += 1;
            }
        }
        let key = unescape(&chars[..end]);
        let value = unescape(&chars[rest..]);
        map.insert(key, value);
    }
    map
}

/// The logical lines: comments and blank lines dropped, continuations joined (the leading
/// whitespace of a continuation line is dropped).
fn logical_lines(text: &str) -> Vec<String> {
    let mut physical = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                physical.push(std::mem::take(&mut current));
            }
            '\n' => physical.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    physical.push(current);

    let mut lines = Vec::new();
    let mut pending: Option<String> = None;
    for raw in physical {
        let line = match pending.take() {
            Some(mut joined) => {
                joined.push_str(raw.trim_start_matches([' ', '\t', '\x0c']));
                joined
            }
            None => {
                let start = raw.trim_start_matches([' ', '\t', '\x0c']);
                if start.is_empty() || start.starts_with('#') || start.starts_with('!') {
                    continue;
                }
                start.to_string()
            }
        };
        let backslashes = line.chars().rev().take_while(|&c| c == '\\').count();
        if backslashes % 2 == 1 {
            let mut joined = line;
            joined.pop();
            pending = Some(joined);
        } else {
            lines.push(line);
        }
    }
    if let Some(joined) = pending {
        lines.push(joined);
    }
    lines
}

fn unescape(chars: &[char]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\\' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&c) = chars.get(i) else { break };
        i += 1;
        match c {
            't' => out.push('\t'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            'f' => out.push('\x0c'),
            'u' => {
                let unit = hex4(chars, i);
                match unit {
                    Some(high) if (0xD800..0xDC00).contains(&high) => {
                        i += 4;
                        let low = (chars.get(i) == Some(&'\\') && chars.get(i + 1) == Some(&'u'))
                            .then(|| hex4(chars, i + 2))
                            .flatten()
                            .filter(|low| (0xDC00..0xE000).contains(low));
                        match low {
                            Some(low) => {
                                i += 6;
                                let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
                                out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                            }
                            None => out.push('\u{fffd}'),
                        }
                    }
                    Some(unit) => {
                        i += 4;
                        out.push(char::from_u32(unit).unwrap_or('\u{fffd}'));
                    }
                    None => out.push('u'),
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn hex4(chars: &[char], at: usize) -> Option<u32> {
    let digits = chars.get(at..at + 4)?;
    digits
        .iter()
        .try_fold(0u32, |acc, c| Some(acc * 16 + c.to_digit(16)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// A class file with one method whose code has `code` bytes.
    fn class_with_code(code: u32, major: u16) -> Vec<u8> {
        let mut c = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0];
        c.extend(major.to_be_bytes());
        c.extend([0, 3]); // pool: 1 = Utf8 Code, 2 = Utf8 m
        c.push(1);
        c.extend([0, 4]);
        c.extend(b"Code");
        c.push(1);
        c.extend([0, 1]);
        c.extend(b"m");
        c.extend([0; 6]); // access, this, super
        c.extend([0, 0, 0, 0]); // interfaces, fields
        c.extend([0, 1]); // methods
        c.extend([0, 1, 0, 2, 0, 2, 0, 1]); // access, name, desc, one attribute
        c.extend([0, 1]);
        c.extend((12 + code).to_be_bytes());
        c.extend([0, 1, 0, 1]);
        c.extend(code.to_be_bytes());
        c.extend(std::iter::repeat_n(0u8, code as usize));
        c.extend([0, 0, 0, 0]);
        c.extend([0, 0]);
        c
    }

    const GOOD_PROPS: &str = "robot.classname=my.Bot\nrobot.version=1.2\nrobot.author.name=Me\nrobot.description=A bot\nrobot.webpage=http://x.example\n";

    fn build(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn good_files() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("my/Bot.class", class_with_code(100, 65)),
            ("my/Bot$1.class", class_with_code(20, 52)),
            ("my/Bot.properties", GOOD_PROPS.as_bytes().to_vec()),
        ]
    }

    fn build_vec(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (*n, d.as_slice())).collect();
        build(&refs)
    }

    fn errors(files: &[(&str, Vec<u8>)]) -> Vec<String> {
        validate(&build_vec(files), &Config::default()).unwrap_err()
    }

    #[test]
    fn a_good_jar() {
        let jar = build_vec(&good_files());
        let info = validate(&jar, &Config::default()).unwrap();
        assert_eq!(info.bot_id, "my.Bot");
        assert_eq!(info.main_class, "my.Bot");
        assert_eq!(info.version, "1.2");
        assert_eq!(info.author, "Me");
        assert_eq!(info.description, "A bot");
        assert_eq!(info.webpage, "http://x.example");
        assert_eq!(info.codesize, 120);
        assert_eq!(info.weight_class, WeightClass::Nano);
        assert_eq!(info.size, jar.len() as u64);
        assert_eq!(info.sha256, sha256_hex(&jar));
        assert_eq!(info.sha256.len(), 64);
        assert_eq!(jar_codesize(&jar, &Config::default()), Ok(120));
    }

    #[test]
    fn author_defaults_to_the_first_package_segment() {
        let mut files = good_files();
        files[2].1 = b"robot.classname=my.Bot\nrobot.version=1\n".to_vec();
        let info = validate(&build_vec(&files), &Config::default()).unwrap();
        assert_eq!(info.author, "my");
        assert_eq!(info.description, "");
    }

    #[test]
    fn weight_classes_are_strictly_below() {
        let mut files = good_files();
        files[0].1 = class_with_code(249, 65);
        files.remove(1);
        let info = validate(&build_vec(&files), &Config::default()).unwrap();
        assert_eq!((info.codesize, info.weight_class), (249, WeightClass::Nano));
        files[0].1 = class_with_code(250, 65);
        let info = validate(&build_vec(&files), &Config::default()).unwrap();
        assert_eq!(
            (info.codesize, info.weight_class),
            (250, WeightClass::Micro)
        );
    }

    #[test]
    fn nested_jars_are_counted_like_classic() {
        let inner = build_vec(&[("lib/Util.class", class_with_code(30, 52))]);
        let mut files = good_files();
        files.push(("lib/util.JAR", inner));
        let info = validate(&build_vec(&files), &Config::default()).unwrap();
        assert_eq!(info.codesize, 150);
    }

    #[test]
    fn no_properties() {
        let files = vec![("my/Bot.class", class_with_code(10, 65))];
        let e = errors(&files);
        assert_eq!(e.len(), 1);
        assert!(e[0].contains("no robot properties"), "{e:?}");
        // A properties file without robot.classname does not count.
        let mut files = files;
        files.push(("lib/x.properties", b"a=b".to_vec()));
        assert!(errors(&files)[0].contains("no robot properties"));
    }

    #[test]
    fn two_properties() {
        let mut files = good_files();
        files.push((
            "my/Other.properties",
            b"robot.classname=my.Other\nrobot.version=1\n".to_vec(),
        ));
        let e = errors(&files);
        assert!(e[0].contains("more than one robot properties"), "{e:?}");
        assert!(e[0].contains("my/Bot.properties") && e[0].contains("my/Other.properties"));
    }

    #[test]
    fn missing_class() {
        let mut files = good_files();
        files.remove(0);
        let e = errors(&files);
        assert!(e[0].contains("my.Bot is not in the jar"), "{e:?}");
        assert!(e[0].contains("my/Bot.class"));
    }

    #[test]
    fn bad_versions() {
        for (props, expect) in [
            ("robot.classname=my.Bot\n", "robot.version is missing"),
            ("robot.classname=my.Bot\nrobot.version=1,0\n", "comma"),
            ("robot.classname=my.Bot\nrobot.version=\n", "missing"),
            (
                "robot.classname=my.Bot\nrobot.version=1.0\\u00e9\n",
                "printable",
            ),
        ] {
            let mut files = good_files();
            files[2].1 = props.as_bytes().to_vec();
            let e = errors(&files);
            assert!(e[0].contains(expect), "{props:?}: {e:?}");
        }
    }

    #[test]
    fn invalid_classname() {
        let mut files = good_files();
        files[2].1 = b"robot.classname=my/Bot\nrobot.version=1\n".to_vec();
        let e = errors(&files);
        assert!(e[0].contains("robot.classname"), "{e:?}");
    }

    #[test]
    fn dangerous_paths() {
        for name in ["../evil.class", "a/../../b.txt", "/abs.txt", "a\\b.txt"] {
            let mut files = good_files();
            files.push((name, vec![1, 2, 3]));
            let e = errors(&files);
            assert_eq!(e.len(), 1, "{name}: {e:?}");
            assert!(e[0].contains(&format!("{name:?}")), "{e:?}");
        }
    }

    #[test]
    fn zip_bomb() {
        let mut files = good_files();
        files.push(("big.dat", vec![0u8; 21 << 20]));
        let jar = build_vec(&files);
        assert!(jar.len() < 100_000, "the bomb must be small: {}", jar.len());
        let e = validate(&jar, &Config::default()).unwrap_err();
        assert!(e[0].contains("unpacks to more than"), "{e:?}");
        // Many entries.
        let mut config = Config::default();
        config.jar.max_entries = 2;
        let e = validate(&build_vec(&good_files()), &config).unwrap_err();
        assert!(e[0].contains("3 entries"), "{e:?}");
    }

    #[test]
    fn team_files() {
        let mut files = good_files();
        files.push(("my/Team.team", b"team.members=a,b".to_vec()));
        let e = errors(&files);
        assert!(e[0].contains("teams are not open yet"), "{e:?}");
    }

    #[test]
    fn bad_magic_and_truncated_classes() {
        let mut files = good_files();
        files[0].1[0] = 0;
        assert!(errors(&files)[0].contains("bad magic"));
        let mut files = good_files();
        files[0].1.truncate(30);
        assert!(errors(&files)[0].contains("my/Bot.class"));
        // A helper class that does not parse also fails the jar.
        let mut files = good_files();
        files[1].1 = vec![1, 2, 3];
        assert!(errors(&files)[0].contains("Bot$1.class"));
        assert!(validate(b"not a zip", &Config::default()).is_err());
    }

    #[test]
    fn too_new_class_files() {
        let mut files = good_files();
        files[0].1 = class_with_code(100, 66);
        let e = errors(&files);
        assert!(e[0].contains("version 66 (Java 22)"), "{e:?}");
    }

    #[test]
    fn too_large() {
        let mut config = Config::default();
        config.jar.max_bytes = 100;
        let e = validate(&build_vec(&good_files()), &config).unwrap_err();
        assert!(e[0].contains("limit is 100 bytes"), "{e:?}");
    }

    #[test]
    fn properties_format() {
        let text = "# comment\n! another\n\n  a=1\nb : two words\nc three\nd\\\n   continued\\\n  twice\nescaped\\ key=v\\u0041\\t\\n\\x\nunicode=\\ud83d\\ude00 \\u00e9\ne=\r\nf==x\n\\#notcomment=1\nlast=y";
        let p = parse_properties(text);
        assert_eq!(p["a"], "1");
        assert_eq!(p["b"], "two words");
        assert_eq!(p["c"], "three");
        assert_eq!(p["dcontinuedtwice"], "");
        assert_eq!(p["escaped key"], "vA\t\nx");
        assert_eq!(p["unicode"], "\u{1F600} \u{e9}");
        assert_eq!(p["e"], "");
        assert_eq!(p["f"], "=x");
        assert_eq!(p["#notcomment"], "1");
        assert_eq!(p["last"], "y");
        // An even number of trailing backslashes is not a continuation.
        let p = parse_properties("a=b\\\\\nc=d");
        assert_eq!(p["a"], "b\\");
        assert_eq!(p["c"], "d");
        // Latin-1 bytes.
        assert_eq!(decode_latin1(&[b'a', 0xE9]), "a\u{e9}");
    }

    fn fixture_rows(file: &str) -> Vec<(String, String, u32)> {
        let path = format!("{}/tests/fixtures/{file}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .map(|line| {
                let mut parts = line.split('\t');
                (
                    parts.next().unwrap().to_string(),
                    parts.next().unwrap().to_string(),
                    parts.next().unwrap().parse().unwrap(),
                )
            })
            .collect()
    }

    /// Classic's codesize tool on the sample robots (jars split out of the samples jar and
    /// checked in under tests/fixtures/samples, EPL-2.0 like the rest of the repository).
    #[test]
    fn codesize_of_the_sample_robots_equals_classics() {
        let rows = fixture_rows("samples.tsv");
        assert!(rows.len() >= 20);
        for (name, sha, expected) in rows {
            let path = format!(
                "{}/tests/fixtures/samples/{name}",
                env!("CARGO_MANIFEST_DIR")
            );
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(sha256_hex(&bytes), sha, "{name}: fixture changed");
            assert_eq!(
                jar_codesize(&bytes, &Config::default()),
                Ok(expected),
                "{name}"
            );
        }
    }

    fn corpus_dir() -> Option<std::path::PathBuf> {
        let dir = std::env::var_os("RUMBLE_CORPUS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/mnt/c/robocode/robots".into());
        dir.is_dir().then_some(dir)
    }

    /// Real RoboRumble jars, when the corpus folder exists (skipped otherwise).
    #[test]
    fn codesize_of_corpus_robots_equals_classics() {
        let Some(dir) = corpus_dir() else {
            eprintln!("no RoboRumble corpus here: skipped");
            return;
        };
        let mut checked = 0;
        for (name, sha, expected) in fixture_rows("corpus.tsv") {
            let Ok(bytes) = std::fs::read(dir.join(&name)) else {
                continue;
            };
            if sha256_hex(&bytes) != sha {
                continue; // another build of the jar
            }
            assert_eq!(
                jar_codesize(&bytes, &Config::default()),
                Ok(expected),
                "{name}"
            );
            checked += 1;
        }
        eprintln!("{checked} corpus jars checked");
    }

    /// Compares every jar of the corpus folder with the output of classic's tool
    /// (`codesize -r <dir>`, path in RUMBLE_CODESIZE_DUMP). Run by hand.
    #[test]
    #[ignore]
    fn whole_corpus_against_a_dump_of_classics_output() {
        let (Some(dir), Ok(dump)) = (corpus_dir(), std::env::var("RUMBLE_CODESIZE_DUMP")) else {
            return;
        };
        let mut checked = 0;
        for line in std::fs::read_to_string(dump).unwrap().lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() != 5 || parts[0].parse::<u32>().is_err() {
                continue;
            }
            let bytes = std::fs::read(dir.join(parts[4])).unwrap();
            let expected: u32 = parts[1].parse().unwrap();
            assert_eq!(
                jar_codesize(&bytes, &Config::default()),
                Ok(expected),
                "{}",
                parts[4]
            );
            checked += 1;
        }
        eprintln!("{checked} jars compared");
    }
}
