#![no_std]
#![no_main]

extern crate alloc;
use libtrios::*;

trios_main!(main);

fn main(args: &[&str]) -> i32 {
    for (i, &arg) in args.iter().enumerate().skip(1) {
        if i > 1 {
            print!(" ");
        }
        print!("{}", arg);
    }
    println!();
    0
}
