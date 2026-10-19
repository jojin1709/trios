#![no_std]
#![no_main]

extern crate alloc;
use libtrios::*;

trios_main!(main);

fn main(_args: &[&str]) -> i32 {
    let secs = uptime();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;

    println!("  PID TTY          TIME CMD");
    println!("    1 ?        {:02}:{:02}:{:02} init", h, m, s);
    println!("    2 ?        {:02}:{:02}:{:02} sh", h, m, s);
    println!("{:5} ?        {:02}:{:02}:{:02} ps", getpid(), h, m, s);
    0
}
