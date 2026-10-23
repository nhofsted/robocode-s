// SPDX-License-Identifier: EPL-2.0

//! The jar cache (§13.5): `<data>/rumble/jars/<sha256>/<id>_<version>.jar`. Jars come from
//! `GET work` with their size and hash; a download is checked against both and tried once
//! more on a mismatch. At start, the least recently used jars go until the cache is below
//! its size limit (last use = the file's modification time, which every use renews).

use rumble_core::api::WorkBot;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use crate::http::sha256_hex;

/// Default size limit of the cache: 2 GiB.
pub const DEFAULT_MAX_BYTES: u64 = 2 << 30;
/// Downloads in parallel.
pub const PARALLEL: usize = 4;

#[derive(Debug)]
pub enum JarError {
    /// The bot's id, version or hash can't name a file.
    BadName(String),
    /// The fetch itself failed (both tries).
    Fetch(String),
    /// Size or hash differ from what the server announced (both tries).
    Mismatch(String),
    Io(io::Error),
}

impl fmt::Display for JarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JarError::BadName(m) => write!(f, "bad jar name: {m}"),
            JarError::Fetch(m) => write!(f, "jar download failed: {m}"),
            JarError::Mismatch(m) => write!(f, "jar hash: {m}"),
            JarError::Io(e) => write!(f, "jar cache: {e}"),
        }
    }
}

impl std::error::Error for JarError {}

impl From<io::Error> for JarError {
    fn from(e: io::Error) -> Self {
        JarError::Io(e)
    }
}

/// What an eviction did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Evicted {
    pub files: usize,
    pub bytes: u64,
}

/// The cache directory.
#[derive(Clone, Debug)]
pub struct JarCache {
    root: PathBuf,
    max_bytes: u64,
}

fn file_name(bot: &WorkBot) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '$') {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    format!("{}_{}.jar", clean(&bot.id), clean(&bot.version))
}

fn valid_sha(sha: &str) -> bool {
    sha.len() == 64
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl JarCache {
    /// A cache in `root` (`<data>/rumble/jars`, see [`JarCache::in_data_dir`]).
    pub fn new(root: impl Into<PathBuf>) -> JarCache {
        JarCache {
            root: root.into(),
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// The cache under the data folder (`Environment::data`).
    pub fn in_data_dir(data: &Path) -> JarCache {
        JarCache::new(data.join("rumble").join("jars"))
    }

    pub fn with_max_bytes(mut self, max_bytes: u64) -> JarCache {
        self.max_bytes = max_bytes;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the bot's jar is (or will be).
    pub fn path(&self, bot: &WorkBot) -> Result<PathBuf, JarError> {
        let sha = bot.sha256.as_str();
        if !valid_sha(sha) {
            return Err(JarError::BadName(format!("sha256 {sha:?}")));
        }
        Ok(self.root.join(sha).join(file_name(bot)))
    }

    /// The cached jar if it is there with the right size (renewing its last-used time), else
    /// `None`.
    pub fn cached(&self, bot: &WorkBot) -> Result<Option<PathBuf>, JarError> {
        let path = self.path(bot)?;
        match fs::metadata(&path) {
            Ok(meta) if meta.is_file() && meta.len() == bot.size => {
                touch(&path);
                Ok(Some(path))
            }
            Ok(_) => {
                // Truncated or replaced: start over.
                let _ = fs::remove_file(&path);
                Ok(None)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The jar's path, downloading it with `fetch` if the cache hasn't it. The download must
    /// have the announced size and SHA-256; a mismatch (or a failed fetch) is tried once more.
    pub fn get<F>(&self, bot: &WorkBot, fetch: &F) -> Result<PathBuf, JarError>
    where
        F: Fn(&WorkBot) -> Result<Vec<u8>, String>,
    {
        if let Some(path) = self.cached(bot)? {
            return Ok(path);
        }
        let path = self.path(bot)?;
        let mut last = None;
        for _ in 0..2 {
            let bytes = match fetch(bot) {
                Ok(b) => b,
                Err(e) => {
                    last = Some(JarError::Fetch(e));
                    continue;
                }
            };
            if bytes.len() as u64 != bot.size {
                last = Some(JarError::Mismatch(format!(
                    "{} {}: {} bytes, expected {}",
                    bot.id,
                    bot.version,
                    bytes.len(),
                    bot.size
                )));
                continue;
            }
            let actual = sha256_hex(&bytes);
            if actual != bot.sha256 {
                last = Some(JarError::Mismatch(format!(
                    "{} {}: sha256 {actual}, expected {}",
                    bot.id, bot.version, bot.sha256
                )));
                continue;
            }
            store(&path, &bytes)?;
            return Ok(path);
        }
        Err(last.unwrap_or_else(|| JarError::Fetch("no attempt".into())))
    }

    /// [`JarCache::get`] for all bots, four at a time; the results are in the order of
    /// `bots`. Bots with the same file are fetched once.
    pub fn ensure<F>(&self, bots: &[WorkBot], fetch: &F) -> Vec<Result<PathBuf, JarError>>
    where
        F: Fn(&WorkBot) -> Result<Vec<u8>, String> + Sync,
    {
        // Distinct files, first occurrence each.
        let mut jobs: Vec<(usize, PathBuf)> = Vec::new();
        let mut results: Vec<Option<Result<PathBuf, JarError>>> =
            bots.iter().map(|_| None).collect();
        for (i, bot) in bots.iter().enumerate() {
            match self.path(bot) {
                Ok(p) if jobs.iter().any(|(_, q)| *q == p) => {}
                Ok(p) => jobs.push((i, p)),
                Err(e) => results[i] = Some(Err(e)),
            }
        }
        let next = AtomicUsize::new(0);
        let done: Mutex<Vec<(usize, Result<PathBuf, JarError>)>> = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..PARALLEL.min(jobs.len()) {
                scope.spawn(|| {
                    loop {
                        let n = next.fetch_add(1, Ordering::Relaxed);
                        let Some((i, _)) = jobs.get(n) else { break };
                        let result = self.get(&bots[*i], fetch);
                        done.lock().unwrap().push((*i, result));
                    }
                });
            }
        });
        let mut by_path: Vec<(PathBuf, Result<PathBuf, JarError>)> = Vec::new();
        for (i, result) in done.into_inner().unwrap() {
            let key = jobs.iter().find(|(j, _)| *j == i).unwrap().1.clone();
            by_path.push((key, result));
        }
        for (i, bot) in bots.iter().enumerate() {
            if results[i].is_some() {
                continue;
            }
            let path = self.path(bot).unwrap();
            let (_, result) = by_path.iter().find(|(p, _)| *p == path).unwrap();
            results[i] = Some(match result {
                Ok(p) => Ok(p.clone()),
                Err(JarError::BadName(m)) => Err(JarError::BadName(m.clone())),
                Err(JarError::Fetch(m)) => Err(JarError::Fetch(m.clone())),
                Err(JarError::Mismatch(m)) => Err(JarError::Mismatch(m.clone())),
                Err(JarError::Io(e)) => Err(JarError::Io(io::Error::new(e.kind(), e.to_string()))),
            });
        }
        results.into_iter().map(Option::unwrap).collect()
    }

    /// Removes least recently used jars until the cache holds at most `max_bytes`. Call it at
    /// start, before any jar is in use.
    pub fn evict(&self) -> io::Result<Evicted> {
        let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        let dirs = match fs::read_dir(&self.root) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Evicted::default()),
            Err(e) => return Err(e),
        };
        for dir in dirs.flatten() {
            if !dir.path().is_dir() {
                continue;
            }
            for file in fs::read_dir(dir.path())?.flatten() {
                let name = file.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".jar") {
                    continue;
                }
                let Ok(meta) = file.metadata() else { continue };
                let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((used, meta.len(), file.path()));
            }
        }
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort();
        let mut evicted = Evicted::default();
        for (_, size, path) in files {
            if total <= self.max_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= size;
                evicted.files += 1;
                evicted.bytes += size;
                if let Some(dir) = path.parent() {
                    let _ = fs::remove_dir(dir); // only if empty
                }
            }
        }
        Ok(evicted)
    }
}

/// Renews the last-used time; failures don't matter.
fn touch(path: &Path) {
    if let Ok(file) = fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// Writes `bytes` to `path` through a temp file and a rename, so a reader never sees half a
/// jar.
fn store(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().expect("jar paths have a directory");
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}.{:?}.tmp",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id(),
        std::thread::current().id()
    ));
    let result = (|| {
        let mut out = fs::File::create(&tmp)?;
        out.write_all(bytes)?;
        out.sync_all()?;
        drop(out);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rumble-jars-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn bot(id: &str, content: &[u8]) -> WorkBot {
        WorkBot {
            id: id.into(),
            version: "1.0".into(),
            vid: 1,
            sha256: sha256_hex(content),
            size: content.len() as u64,
            url: "/jars/x.jar".into(),
        }
    }

    #[test]
    fn downloads_verifies_and_caches() {
        let dir = temp("get");
        let cache = JarCache::new(dir.join("jars"));
        let b = bot("a.B", b"jar bytes");
        let calls = AtomicU32::new(0);
        let fetch = |_: &WorkBot| {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(b"jar bytes".to_vec())
        };
        let path = cache.get(&b, &fetch).unwrap();
        assert_eq!(path, dir.join("jars").join(&b.sha256).join("a.B_1.0.jar"));
        assert_eq!(fs::read(&path).unwrap(), b"jar bytes");
        // Second time: from the cache.
        assert_eq!(cache.get(&b, &fetch).unwrap(), path);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        // No temp files left.
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mismatch_is_retried_once() {
        let dir = temp("retry");
        let cache = JarCache::new(dir.join("jars"));
        let b = bot("a.B", b"good");
        let calls = AtomicU32::new(0);
        // Wrong bytes first, then right.
        let flaky = |_: &WorkBot| {
            Ok(if calls.fetch_add(1, Ordering::Relaxed) == 0 {
                b"evil".to_vec()
            } else {
                b"good".to_vec()
            })
        };
        assert!(cache.get(&b, &flaky).is_ok());
        assert_eq!(calls.load(Ordering::Relaxed), 2);

        // Always wrong: two tries, then a mismatch, and nothing cached.
        let c = bot("c.D", b"good");
        let calls = AtomicU32::new(0);
        let bad = |_: &WorkBot| {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(b"bad!".to_vec())
        };
        match cache.get(&c, &bad) {
            Err(JarError::Mismatch(m)) => assert!(m.contains("sha256"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert!(!cache.path(&c).unwrap().exists());

        // Wrong size is a mismatch too; fetch errors are retried and reported.
        let short = |_: &WorkBot| Ok(b"go".to_vec());
        assert!(matches!(cache.get(&c, &short), Err(JarError::Mismatch(_))));
        let down = |_: &WorkBot| Err("503".to_string());
        assert!(matches!(cache.get(&c, &down), Err(JarError::Fetch(m)) if m == "503"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn damaged_cache_entry_is_fetched_again() {
        let dir = temp("damaged");
        let cache = JarCache::new(dir.join("jars"));
        let b = bot("a.B", b"jar bytes");
        let fetch = |_: &WorkBot| Ok(b"jar bytes".to_vec());
        let path = cache.get(&b, &fetch).unwrap();
        fs::write(&path, b"jar").unwrap();
        cache.get(&b, &fetch).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"jar bytes");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bad_names_are_refused() {
        let cache = JarCache::new("/nonexistent/jars");
        let mut b = bot("a.B", b"x");
        b.sha256 = "../../etc".into();
        assert!(matches!(cache.path(&b), Err(JarError::BadName(_))));
        let mut b = bot("../evil", b"x");
        b.version = "1/2".into();
        assert_eq!(
            cache.path(&b).unwrap().file_name().unwrap(),
            ".._evil_1_2.jar"
        );
    }

    #[test]
    fn ensure_downloads_four_at_a_time() {
        let dir = temp("parallel");
        let cache = JarCache::new(dir.join("jars"));
        let contents: Vec<Vec<u8>> = (0..10)
            .map(|i| format!("jar number {i}").into_bytes())
            .collect();
        let mut bots: Vec<WorkBot> = contents
            .iter()
            .enumerate()
            .map(|(i, c)| bot(&format!("bot.N{i}"), c))
            .collect();
        // The same jar twice, and one that can't be named.
        bots.push(bots[0].clone());
        let mut broken = bot("x.Y", b"q");
        broken.sha256 = "nope".into();
        bots.push(broken);

        let running = AtomicU32::new(0);
        let peak = AtomicU32::new(0);
        let fetched = AtomicU32::new(0);
        let fetch = |b: &WorkBot| {
            fetched.fetch_add(1, Ordering::Relaxed);
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(60));
            running.fetch_sub(1, Ordering::SeqCst);
            let i: usize = b.id.trim_start_matches("bot.N").parse().unwrap();
            Ok(contents[i].clone())
        };
        let results = cache.ensure(&bots, &fetch);
        assert_eq!(results.len(), 12);
        for r in &results[..11] {
            assert!(r.is_ok(), "{r:?}");
        }
        assert_eq!(results[10].as_ref().unwrap(), results[0].as_ref().unwrap());
        assert!(matches!(results[11], Err(JarError::BadName(_))));
        assert_eq!(fetched.load(Ordering::SeqCst), 10);
        assert_eq!(peak.load(Ordering::SeqCst), 4);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn evicts_least_recently_used_first() {
        let dir = temp("lru");
        let cache = JarCache::new(dir.join("jars")).with_max_bytes(25);
        let fetch_for = |content: &'static [u8]| move |_: &WorkBot| Ok(content.to_vec());
        let contents: [&'static [u8]; 3] = [b"aaaaaaaaaa", b"bbbbbbbbbb", b"cccccccccc"];
        let bots: Vec<WorkBot> = contents
            .iter()
            .enumerate()
            .map(|(i, c)| bot(&format!("bot.N{i}"), c))
            .collect();
        let paths: Vec<PathBuf> = bots
            .iter()
            .zip(contents)
            .map(|(b, c)| cache.get(b, &fetch_for(c)).unwrap())
            .collect();
        // Last used: 1 oldest, then 2, then 0 (just used again).
        let now = SystemTime::now();
        let set = |p: &Path, ago: u64| {
            let f = fs::OpenOptions::new().write(true).open(p).unwrap();
            f.set_modified(now - Duration::from_secs(ago)).unwrap();
        };
        set(&paths[0], 100);
        set(&paths[1], 300);
        set(&paths[2], 200);
        assert!(cache.cached(&bots[0]).unwrap().is_some()); // renews 0
        let evicted = cache.evict().unwrap();
        assert_eq!(
            evicted,
            Evicted {
                files: 1,
                bytes: 10
            }
        );
        assert!(!paths[1].exists());
        assert!(!paths[1].parent().unwrap().exists());
        assert!(paths[0].exists() && paths[2].exists());

        // Under the limit: nothing happens; a smaller limit removes the next oldest (2).
        assert_eq!(cache.evict().unwrap(), Evicted::default());
        let cache = cache.with_max_bytes(10);
        cache.evict().unwrap();
        assert!(paths[0].exists() && !paths[2].exists());
        // No cache directory at all is fine.
        assert_eq!(
            JarCache::new(dir.join("none")).evict().unwrap(),
            Evicted::default()
        );
        let _ = fs::remove_dir_all(dir);
    }
}
