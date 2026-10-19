#![no_std]
#![no_main]

extern crate alloc;
use libtrios::*;

trios_main!(main);

fn main(args: &[&str]) -> i32 {
    println!("Hello from Ring 3 userland! PID: {}, Uptime: {}s", getpid(), uptime());
    if args.len() > 1 {
        print!("Arguments:");
        for a in &args[1..] {
            print!(" {}", a);
        }
        println!();
    }
    0
}
