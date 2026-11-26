//! Replays the title's streaming requests from a `VICTORY_DSTORAGE_LOG` file
//! against the game's files, one thread, and times each stage apart: the
//! file read, zlib, BCPack. No game run, no GPU.
//!
//! `cargo run --release -p victory-hle --example stream_bench -- <log> <game dir> [per kind]`

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

const ZLIB: u64 = 1;
const BCPACK: u64 = 0xf << 1;
const PAGE_DESTINATION: u64 = 1 << 21;
const MEMORY_SOURCE: u64 = 1 << 22;

struct Req {
    file: String,
    offset: u64,
    src: usize,
    dst: usize,
    options: u64,
}

fn inflate(state: &mut zlib_rs::Inflate, source: &[u8], out: &mut [u8]) -> Option<usize> {
    state.reset(true);
    let (mut in_pos, mut out_pos) = (0usize, 0usize);
    loop {
        match state.decompress(&source[in_pos..], &mut out[out_pos..], zlib_rs::InflateFlush::NoFlush) {
            Ok(zlib_rs::Status::StreamEnd) => return Some(state.total_out() as usize),
            Ok(zlib_rs::Status::Ok) => {
                let (i, o) = (state.total_in() as usize, state.total_out() as usize);
                if i == in_pos && o == out_pos {
                    return None;
                }
                (in_pos, out_pos) = (i, o);
            }
            _ => return None,
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (log, game) = (&args[1], std::path::PathBuf::from(&args[2]));
    let per_kind: usize = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(2000);
    let text = std::fs::read_to_string(log).expect("log");
    let mut files: HashMap<String, String> = HashMap::new();
    let mut by_kind: HashMap<&'static str, Vec<Req>> = HashMap::new();
    for line in text.lines() {
        let p: Vec<&str> = line.split('\t').collect();
        match p.first() {
            Some(&"F") if p.len() >= 3 => {
                files.insert(p[1].to_owned(), p[2].trim_start_matches("G:\\").to_owned());
            }
            Some(&"R") if p.len() >= 8 => {
                let options = u64::from_str_radix(p[7], 16).unwrap_or(0);
                if options & MEMORY_SOURCE != 0 {
                    continue;
                }
                let Some(file) = files.get(p[3]) else { continue };
                let kind = match (options & BCPACK != 0, options & ZLIB != 0) {
                    (true, true) => "zlib+bcpack",
                    (true, false) => "bcpack",
                    (false, true) => "zlib",
                    _ => "raw",
                };
                by_kind.entry(kind).or_default().push(Req {
                    file: file.clone(),
                    offset: p[4].parse().unwrap_or(0),
                    src: p[5].parse().unwrap_or(0),
                    dst: p[6].parse().unwrap_or(0),
                    options,
                });
            }
            _ => {}
        }
    }
    let tables = victory_formats::bcpack::Tables::builtin().expect("BCPack tables");
    let mut state = zlib_rs::Inflate::new(true, 15);
    let mut open: HashMap<String, std::fs::File> = HashMap::new();
    for (kind, reqs) in &by_kind {
        // Spread over the whole run.
        let step = (reqs.len() / per_kind).max(1);
        let sample: Vec<&Req> = reqs.iter().step_by(step).take(per_kind).collect();
        let (mut read_t, mut work_t) = (Duration::ZERO, Duration::ZERO);
        let (mut src_b, mut dst_b, mut failed, mut pages) = (0usize, 0usize, 0usize, 0usize);
        for r in &sample {
            let f = open.entry(r.file.clone()).or_insert_with(|| std::fs::File::open(game.join(&r.file)).expect("game file"));
            let mut src = vec![0u8; r.src];
            let t = Instant::now();
            if f.seek(SeekFrom::Start(r.offset)).is_err() || f.read_exact(&mut src).is_err() {
                failed += 1;
                continue;
            }
            read_t += t.elapsed();
            let mut dst = vec![0u8; r.dst];
            let t = Instant::now();
            let ok = if r.options & BCPACK != 0 {
                let bc = ((r.options & BCPACK) >> 1) as u8;
                victory_formats::bcpack::decode(bc, &src, &mut dst, &tables).is_ok()
            } else if r.options & ZLIB != 0 {
                inflate(&mut state, &src, &mut dst).is_some()
            } else {
                dst[..r.src.min(r.dst)].copy_from_slice(&src[..r.src.min(r.dst)]);
                true
            };
            work_t += t.elapsed();
            failed += usize::from(!ok);
            pages += usize::from(r.options & PAGE_DESTINATION != 0);
            src_b += r.src;
            dst_b += r.dst;
        }
        let n = sample.len().max(1);
        println!(
            "{kind:12} of {:6}: {n:5} sampled ({pages} to pages, {failed} failed) | read {:7.1} ms ({:.3} ms each, {:6.0} MB/s) | decode {:7.1} ms ({:.3} ms each, {:6.0} MB/s out)",
            reqs.len(),
            read_t.as_secs_f64() * 1e3,
            read_t.as_secs_f64() * 1e3 / n as f64,
            src_b as f64 / 1e6 / read_t.as_secs_f64().max(1e-9),
            work_t.as_secs_f64() * 1e3,
            work_t.as_secs_f64() * 1e3 / n as f64,
            dst_b as f64 / 1e6 / work_t.as_secs_f64().max(1e-9),
        );
    }
}
