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
}
