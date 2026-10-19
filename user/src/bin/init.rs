#![no_std]
#![no_main]

extern crate alloc;
use libtrios::*;

trios_main!(main);

fn main(_args: &[&str]) -> i32 {
    println!("[init] TriOS PID 1 init starting...");
    println!("[init] System uptime: {}s, PID: {}", uptime(), getpid());

    // Spawn user shell /bin/sh
    let sh_pid = spawn("/bin/sh", &["/bin/sh"]);
    if sh_pid <= 0 {
        eprintln!("[init] Failed to spawn /bin/sh (status: {sh_pid})");
        return 1;
    }
    println!("[init] /bin/sh spawned successfully as PID #{sh_pid}");

    // Supervise children
    loop {
        let reaped_code = wait(0);
        println!("[init] Reaped child process (status {reaped_code})");
        sleep_ms(500);
        let next_sh = spawn("/bin/sh", &["/bin/sh"]);
        if next_sh > 0 {
            println!("[init] Respawned /bin/sh as PID #{next_sh}");
        }
    }
}
