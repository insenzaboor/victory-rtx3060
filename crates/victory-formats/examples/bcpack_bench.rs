//! Decodes a corpus written by `victory-hle`'s `bcpack_corpus` example on one
//! thread, checks every output against the hash stored with it, and prints
//! the throughput per BC format and in all.
//!
//! `cargo run --release -p victory-formats --example bcpack_bench -- <corpus dir> [rounds]`

use std::time::{Duration, Instant};

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let rounds: usize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(3);
    let mut corpus = Vec::new();
    let mut names: Vec<_> = std::fs::read_dir(&dir).expect("corpus dir").flatten().map(|e| e.path()).collect();
    names.sort();
    for path in names {
        let bytes = std::fs::read(&path).expect("corpus file");
        let bc = bytes[0];
        let dst = u32::from_le_bytes(bytes[1..5].try_into().unwrap()) as usize;
        let src = u32::from_le_bytes(bytes[5..9].try_into().unwrap()) as usize;
        let source = bytes[9..9 + src].to_vec();
        let hash = u64::from_le_bytes(bytes[9 + src..9 + src + 8].try_into().unwrap());
        corpus.push((bc, dst, source, hash));
    }
    let tables = victory_formats::bcpack::Tables::builtin().expect("tables");
    let mut by_bc: std::collections::BTreeMap<u8, (Duration, usize, usize)> = Default::default();
    let mut wrong = 0;
    let mut best = Duration::MAX;
    for round in 0..rounds {
        let mut all = Duration::ZERO;
        for (bc, dst_len, src, hash) in &corpus {
            let mut dst = vec![0u8; *dst_len];
            let t = Instant::now();
            let ok = victory_formats::bcpack::decode(*bc, src, &mut dst, &tables).is_ok();
            let took = t.elapsed();
            all += took;
            if round == 0 && (!ok || fnv(&dst) != *hash) {
                wrong += 1;
            }
            if round + 1 == rounds {
                let e = by_bc.entry(*bc).or_default();
                e.0 += took;
                e.1 += 1;
                e.2 += dst_len;
            }
        }
        best = best.min(all);
    }
    let out: usize = corpus.iter().map(|c| c.1).sum();
    println!(
        "lanes (entropy decode) {:.1} ms over {rounds} rounds",
        victory_formats::bcpack::PROFILE_LANES_NS.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6
    );
    for (bc, (t, n, bytes)) in &by_bc {
        println!(
            "BC{bc}: {n:4} requests, {:7.3} ms each, {:6.1} MB/s out",
            t.as_secs_f64() * 1e3 / *n as f64,
            *bytes as f64 / 1e6 / t.as_secs_f64()
        );
    }
    println!(
        "all: {} requests, {:.1} MB out, best round {:.1} ms = {:.3} ms each, {:.1} MB/s; {wrong} wrong",
        corpus.len(),
        out as f64 / 1e6,
        best.as_secs_f64() * 1e3,
        best.as_secs_f64() * 1e3 / corpus.len() as f64,
        out as f64 / 1e6 / best.as_secs_f64()
    );
}
