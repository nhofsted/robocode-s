// SPDX-License-Identifier: EPL-2.0

//! Listings of the robots at once (two GUIs listing at the same time) don't break each
//! other. Needs Java and the built robot host and sample robots, so it is ignored by
//! default: run it with `cargo test -p robocode-s-runner -- --ignored`.

use robocode_s_runner::local::{Environment, list_robots};
use std::path::Path;
use std::thread;

#[test]
#[ignore = "needs Java and the built robots"]
fn listings_at_once_each_have_a_home_of_their_own() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = Environment::from_env();
    for path in [&mut env.host_lib, &mut env.agent_jar, &mut env.data] {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    }
    env.jars = env.jars.iter().map(|j| root.join(j)).collect();
    let host = env.host(false).unwrap();
    let dir = env.data.join(format!("list-test-{}", std::process::id()));
    let listings: Vec<_> = (0..4)
        .map(|_| {
            let (host, jars, dir) = (host.clone(), env.jars.clone(), dir.clone());
            thread::spawn(move || list_robots(&host, &jars, &dir))
        })
        .collect();
    let lists: Vec<_> = listings
        .into_iter()
        .map(|l| l.join().unwrap().unwrap())
        .collect();
    assert!(lists[0].iter().any(|r| r.name.starts_with("sample.Fire")));
    for list in &lists {
        assert_eq!(list, &lists[0]);
    }
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert!(left.is_empty(), "each listing removes its home");
    std::fs::remove_dir_all(&dir).unwrap();
    let _ = std::fs::remove_file(cache_of(&dir));
}

/// Where a listing below `dir` caches its jars' robots.
fn cache_of(dir: &Path) -> std::path::PathBuf {
    let mut name = dir.as_os_str().to_owned();
    name.push(".cache.tsv");
    name.into()
}

#[test]
#[ignore = "needs Java and the built robots"]
fn a_cached_listing_is_the_same_as_a_new_one() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = Environment::from_env();
    for path in [&mut env.host_lib, &mut env.agent_jar, &mut env.data] {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    }
    env.jars = env.jars.iter().map(|j| root.join(j)).collect();
    let host = env.host(false).unwrap();
    let dir = env
        .data
        .join(format!("list-cache-test-{}", std::process::id()));
    let _ = std::fs::remove_file(cache_of(&dir));
    let new = list_robots(&host, &env.jars, &dir).unwrap();
    assert!(cache_of(&dir).is_file());
    let cached = list_robots(&host, &env.jars, &dir).unwrap();
    assert_eq!(cached, new);
    let fire = new
        .iter()
        .find(|r| r.name.starts_with("sample.Fire"))
        .unwrap();
    assert!(
        fire.source
            .as_ref()
            .unwrap()
            .ends_with("samples-0.1.0-SNAPSHOT.jar")
    );
    std::fs::remove_file(cache_of(&dir)).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
