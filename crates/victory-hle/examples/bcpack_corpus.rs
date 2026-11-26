//! Writes a corpus of the title's BCPack requests for `victory-formats`'
//! `bcpack_bench`: from a `VICTORY_DSTORAGE_LOG` file and the game's files,
//! up to N requests (inflated first where zlib-packed), one file each:
//! `bc` (u8), destination bytes (u32), source bytes (u32), the source, then
//! the 64-bit FNV-1a hash of what today's decoder makes of it.
//!
//! `cargo run --release -p victory-hle --example bcpack_corpus -- <log> <game dir> <out dir> [N]`

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};

const ZLIB: u64 = 1;
const BCPACK: u64 = 0xf << 1;
const MEMORY_SOURCE: u64 = 1 << 22;

fn inflate(state: &mut zlib_rs::Inflate, source: &[u8], out: &mut Vec<u8>) -> bool {
    state.reset(true);
    let (mut in_pos, mut out_pos) = (0usize, 0usize);
    loop {
        if out_pos == out.len() {
            out.resize(out.len() * 2 + 4096, 0);
        }
        match state.decompress(&source[in_pos..], &mut out[out_pos..], zlib_rs::InflateFlush::NoFlush) {
            Ok(zlib_rs::Status::StreamEnd) => {
                out.truncate(state.total_out() as usize);
                return true;
            }
            Ok(zlib_rs::Status::Ok) | Ok(zlib_rs::Status::BufError) => {
                let (i, o) = (state.total_in() as usize, state.total_out() as usize);
                if i == in_pos && o == out_pos && o < out.len() {
                    return false;
                }
                (in_pos, out_pos) = (i, o);
            }
            _ => return false,
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (log, game, out) = (&args[1], std::path::PathBuf::from(&args[2]), std::path::PathBuf::from(&args[3]));
    let most: usize = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(400);
    std::fs::create_dir_all(&out).expect("out dir");
    let text = std::fs::read_to_string(log).expect("log");
    let mut files: HashMap<String, String> = HashMap::new();
    let mut reqs = Vec::new();
    for line in text.lines() {
        let p: Vec<&str> = line.split('\t').collect();
        match p.first() {
            Some(&"F") if p.len() >= 3 => {
                files.insert(p[1].to_owned(), p[2].trim_start_matches("G:\\").to_owned());
            }
            Some(&"R") if p.len() >= 8 => {
                let options = u64::from_str_radix(p[7], 16).unwrap_or(0);
                if options & MEMORY_SOURCE != 0 || options & BCPACK == 0 {
                    continue;
                }
                let Some(file) = files.get(p[3]) else { continue };
                let (offset, src, dst): (u64, usize, usize) =
                    (p[4].parse().unwrap_or(0), p[5].parse().unwrap_or(0), p[6].parse().unwrap_or(0));
                reqs.push((file.clone(), offset, src, dst, options));
            }
            _ => {}
        }
    }
    let step = (reqs.len() / most).max(1);
    let tables = victory_formats::bcpack::Tables::builtin().expect("tables");
    let mut state = zlib_rs::Inflate::new(true, 15);
    let mut open: HashMap<String, std::fs::File> = HashMap::new();
    let (mut written, mut failed) = (0, 0);
    for (n, (file, offset, src_len, dst_len, options)) in reqs.iter().step_by(step).take(most).enumerate() {
        let f = open.entry(file.clone()).or_insert_with(|| std::fs::File::open(game.join(file)).expect("game file"));
        let mut src = vec![0u8; *src_len];
        if f.seek(SeekFrom::Start(*offset)).is_err() || f.read_exact(&mut src).is_err() {
            failed += 1;
            continue;
        }
        if options & ZLIB != 0 {
            let mut inflated = vec![0u8; dst_len * 2];
            if !inflate(&mut state, &src, &mut inflated) {
                failed += 1;
                continue;
            }
            src = inflated;
        }
        let bc = ((options & BCPACK) >> 1) as u8;
        let mut dst = vec![0u8; *dst_len];
        if victory_formats::bcpack::decode(bc, &src, &mut dst, &tables).is_err() {
            failed += 1;
            continue;
        }
        let hash = dst.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)).to_le_bytes();
        let mut o = std::fs::File::create(out.join(format!("{n:05}.bcp"))).expect("corpus file");
        o.write_all(&[bc]).unwrap();
        o.write_all(&(*dst_len as u32).to_le_bytes()).unwrap();
        o.write_all(&(src.len() as u32).to_le_bytes()).unwrap();
        o.write_all(&src).unwrap();
        o.write_all(&hash).unwrap();
        written += 1;
    }
    println!("{} BCPack requests in the log; {written} written, {failed} failed", reqs.len());
}
