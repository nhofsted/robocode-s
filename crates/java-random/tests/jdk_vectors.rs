// SPDX-License-Identifier: EPL-2.0

//! Replays test vectors produced by a real JDK (see `tools/difftest/.../JavaRandomVectors.java`).

use robocode_s_java_random::{JavaRandom, shuffle};

const VECTORS: &str = include_str!("data/java_random.txt");

#[test]
fn matches_jdk_vectors() {
    let mut random: Option<JavaRandom> = None;
    let mut checked = 0;
    for (index, line) in VECTORS.lines().enumerate() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let at = || format!("line {}: {line}", index + 1);
        let fields: Vec<&str> = line.split(' ').collect();
        if fields[0] == "seed" {
            random = Some(JavaRandom::new(fields[1].parse().unwrap()));
            continue;
        }
        let r = random.as_mut().expect("operation before first seed");
        match fields[0] {
            "setSeed" => r.set_seed(fields[1].parse().unwrap()),
            "nextInt" => assert_eq!(r.next_int(), fields[1].parse::<i32>().unwrap(), "{}", at()),
            "nextIntBounded" => {
                let bound = fields[1].parse().unwrap();
                assert_eq!(
                    r.next_int_bounded(bound),
                    fields[2].parse::<i32>().unwrap(),
                    "{}",
                    at()
                );
            }
            "nextLong" => assert_eq!(r.next_long(), fields[1].parse::<i64>().unwrap(), "{}", at()),
            "nextBoolean" => assert_eq!(
                r.next_boolean(),
                fields[1].parse::<bool>().unwrap(),
                "{}",
                at()
            ),
            "nextFloat" => {
                let bits = u32::from_str_radix(fields[1], 16).unwrap();
                assert_eq!(r.next_float().to_bits(), bits, "{}", at());
            }
            "nextDouble" => {
                let bits = u64::from_str_radix(fields[1], 16).unwrap();
                assert_eq!(r.next_double().to_bits(), bits, "{}", at());
            }
            "nextGaussian" => {
                let bits = u64::from_str_radix(fields[1], 16).unwrap();
                assert_eq!(r.next_gaussian().to_bits(), bits, "{}", at());
            }
            "nextBytes" => {
                let mut bytes = vec![0u8; fields[1].parse().unwrap()];
                r.next_bytes(&mut bytes);
                let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(
                    if hex.is_empty() { "-" } else { &hex },
                    fields[2],
                    "{}",
                    at()
                );
            }
            "shuffle" => {
                let size: usize = fields[1].parse().unwrap();
                let mut list: Vec<usize> = (0..size).collect();
                shuffle(&mut list, r);
                let order: Vec<String> = list.iter().map(ToString::to_string).collect();
                let order = order.join(",");
                assert_eq!(
                    if order.is_empty() { "-" } else { &order },
                    fields[2],
                    "{}",
                    at()
                );
            }
            other => panic!("unknown operation {other} at {}", at()),
        }
        checked += 1;
    }
    assert!(checked > 3000, "only {checked} vectors checked");
}
