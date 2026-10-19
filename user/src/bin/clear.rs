#![no_std]
#![no_main]

extern crate alloc;
use libtrios::*;

trios_main!(main);

fn main(_args: &[&str]) -> i32 {
    print!("\x1b[2J\x1b[H\x0c");
    0
}
