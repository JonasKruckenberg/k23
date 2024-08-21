//! This abomination of a utility is simple wrapper around the lz4_flex crate to
//! compress using its block format not the default framed format. This is only
//! necessary because the lz4_flex crates no_std mode does not support the framed format
//! and I can't be bothered to port the whole thing to no_std right now.

use lz4_flex::compress_prepend_size;
use std::{env, fs};

fn main() {
    let mut args = env::args().skip(1);
    let inpath = args.next().expect("missing inpath argument");
    let outpath = args.next().expect("missing outpath argument");

    let input = fs::read(&inpath).expect("failed to read file");
    let compressed = compress_prepend_size(&input);
    fs::write(outpath, &compressed).expect("failed to write file");
}
