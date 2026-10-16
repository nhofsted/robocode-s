// SPDX-License-Identifier: EPL-2.0

//! Encodes every round of difftest recordings, checks the round trip, and reports sizes.
//!
//! cargo run --release -p robocode-s-replay --features json --bin replay-size -- recordings/*.jsonl.gz
//!
//! With `--write DIR` first, it also writes each round's track to `DIR/<name>-<round>.track`
//! (one round per file, as the rumble stores them; the desktop GUI's Replay Battle opens them).

use robocode_s_replay::{LABELS, POS_SCALE, decode, encode_with_costs, recording};
use std::path::Path;

fn main() {
    let mut total_bytes = 0usize;
    let mut total_turns = 0usize;
    let mut total_costs = vec![0.0; LABELS.len()];
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let write = match args.first().map(String::as_str) {
        Some("--write") if args.len() > 1 => {
            let dir = std::path::PathBuf::from(args.remove(1));
            args.remove(0);
            std::fs::create_dir_all(&dir).expect("creates the output folder");
            Some(dir)
        }
        _ => None,
    };
    for arg in args {
        let lines = recording::load(Path::new(&arg));
        let tracks = recording::tracks(&lines);
        let flights = recording::bullet_positions(&lines);
        let name = Path::new(&arg)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let robots = tracks[0].frames[0].len();
        let (mut bytes, mut turns) = (0, 0);
        let mut worst = 0.0f64;
        for (round, (track, flight)) in tracks.iter().zip(&flights).enumerate() {
            let (data, costs) = encode_with_costs(track);
            if let Some(dir) = &write {
                let stem = name.split('.').next().unwrap_or(&name);
                let file = dir.join(format!("{stem}-{}.track", round + 1));
                std::fs::write(&file, &data).expect("writes the track");
            }
            let back = decode(&data).expect("decodes");
            assert!(back == *track, "{name}: round trip differs");
            bytes += data.len();
            turns += track.frames.len() - 1;
            for (t, c) in total_costs.iter_mut().zip(&costs) {
                *t += c;
            }
            for &(i, t, x, y) in flight {
                let (px, py) = track.bullets[i].position(t);
                worst = worst.max((px - x).hypot(py - y) / POS_SCALE as f64);
            }
        }
        println!(
            "{name:32} {robots:2} robots {:2} rounds {turns:6} turns {bytes:8} B  {:6.0} B/round  {:5.2} B/turn  bullet path error ≤ {worst:.2} px",
            tracks.len(),
            bytes as f64 / tracks.len() as f64,
            bytes as f64 / turns as f64,
        );
        total_bytes += bytes;
        total_turns += turns;
    }
    println!(
        "\ntotal {total_bytes} B over {total_turns} turns: {:.2} B/turn",
        total_bytes as f64 / total_turns as f64
    );
    let bits: f64 = total_costs.iter().sum();
    for (l, c) in LABELS.iter().zip(&total_costs) {
        println!(
            "  {l:13} {:5.1}%  {:5.2} B/turn",
            100.0 * c / bits,
            c / 8.0 / total_turns as f64
        );
    }
}
