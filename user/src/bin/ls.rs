#![no_std]
#![no_main]

extern crate alloc;
use libtrios::*;

trios_main!(main);

fn main(args: &[&str]) -> i32 {
    let mut cwd_buf = [0u8; 128];
    let path = if args.len() > 1 {
        args[1]
    } else {
        let n = getcwd(&mut cwd_buf);
        if n > 0 {
            core::str::from_utf8(&cwd_buf[..n as usize]).unwrap_or("/")
        } else {
            "/"
        }
    };

    let mut dir_buf = [0u8; 2048];
    let n = readdir(path, &mut dir_buf);
    if n < 0 {
        eprintln!("ls: cannot access '{}': No such directory", path);
        return 1;
    }

    if let Ok(content) = core::str::from_utf8(&dir_buf[..n as usize]) {
        for entry in content.lines() {
            let entry = entry.trim();
            if !entry.is_empty() {
                println!("{}", entry);
            }
        }
    }
    0
}
