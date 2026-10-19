#![no_std]
#![no_main]

extern crate alloc;
use libtrios::*;

trios_main!(main);

fn cat_fd(fd: usize) {
    let mut buf = [0u8; 512];
    loop {
        let n = read(fd, &mut buf);
        if n <= 0 {
            break;
        }
        write(1, &buf[..n as usize]);
    }
}

fn main(args: &[&str]) -> i32 {
    if args.len() <= 1 {
        // Read from stdin (supporting pipes)
        cat_fd(0);
        return 0;
    }

    let mut exit_code = 0;
    for &path in &args[1..] {
        let fd = open(path, 0);
        if fd < 0 {
            eprintln!("cat: {}: No such file or directory", path);
            exit_code = 1;
            continue;
        }
        cat_fd(fd as usize);
        let _ = close(fd as usize);
    }
    exit_code
}
