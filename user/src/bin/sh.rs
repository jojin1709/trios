#![no_std]
#![no_main]

extern crate alloc;
use libtrios::*;

trios_main!(main);

fn read_line(buf: &mut [u8]) -> usize {
    let mut pos = 0;
    while pos < buf.len() {
        let mut b = [0u8; 1];
        let n = read(0, &mut b);
        if n <= 0 {
            sleep_ms(10);
            continue;
        }
        let ch = b[0];
        if ch == b'\n' || ch == b'\r' {
            break;
        }
        if ch == 8 || ch == 127 {
            if pos > 0 {
                pos -= 1;
            }
            continue;
        }
        buf[pos] = ch;
        pos += 1;
    }
    pos
}

fn split_words<'a>(s: &'a str, storage: &mut [&'a str; 16]) -> usize {
    let mut count = 0;
    for word in s.split_whitespace() {
        if count < storage.len() {
            storage[count] = word;
            count += 1;
        }
    }
    count
}

fn execute_command(line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }

    // Check for pipe '|'
    if let Some((left, right)) = trimmed.split_once('|') {
        let left = left.trim();
        let right = right.trim();
        execute_pipe(left, right);
        return;
    }

    // Check for redirection '>'
    if let Some((cmd, out_file)) = trimmed.split_once('>') {
        let cmd = cmd.trim();
        let out_file = out_file.trim();
        execute_redirect(cmd, out_file);
        return;
    }

    let mut token_storage = [""; 16];
    let token_count = split_words(trimmed, &mut token_storage);
    if token_count == 0 {
        return;
    }

    let cmd = token_storage[0];
    let args = &token_storage[..token_count];

    // Builtins
    if cmd == "cd" {
        let target = if token_count > 1 { token_storage[1] } else { "/" };
        if chdir(target) != 0 {
            eprintln!("cd: {}: No such file or directory", target);
        }
        return;
    }

    if cmd == "exit" {
        println!("exit");
        exit(0);
    }

    // Resolve binary path
    let mut resolved_buf = [0u8; 128];
    let binary_path = if cmd.starts_with('/') {
        cmd
    } else {
        let prefix = b"/bin/";
        let cmd_bytes = cmd.as_bytes();
        if prefix.len() + cmd_bytes.len() < resolved_buf.len() {
            resolved_buf[..prefix.len()].copy_from_slice(prefix);
            resolved_buf[prefix.len()..prefix.len() + cmd_bytes.len()].copy_from_slice(cmd_bytes);
            core::str::from_utf8(&resolved_buf[..prefix.len() + cmd_bytes.len()]).unwrap_or(cmd)
        } else {
            cmd
        }
    };

    let pid = spawn(binary_path, args);
    if pid <= 0 {
        eprintln!("{}: command not found", cmd);
    } else {
        let _ = wait(pid as usize);
    }
}

fn execute_pipe(cmd1: &str, cmd2: &str) {
    let mut pipe_fds = [0i32; 2];
    if pipe(&mut pipe_fds) != 0 {
        eprintln!("pipe creation failed");
        return;
    }

    let saved_stdout = 10;
    let saved_stdin = 11;
    let _ = dup2(1, saved_stdout);
    let _ = dup2(0, saved_stdin);

    // Redirect stdout -> pipe write end
    let _ = dup2(pipe_fds[1] as usize, 1);
    let mut tokens1 = [""; 16];
    let count1 = split_words(cmd1, &mut tokens1);
    let pid1 = if count1 > 0 {
        let cmd = tokens1[0];
        let mut buf = [0u8; 64];
        let path = if cmd.starts_with('/') {
            cmd
        } else {
            let prefix = b"/bin/";
            buf[..prefix.len()].copy_from_slice(prefix);
            buf[prefix.len()..prefix.len() + cmd.len()].copy_from_slice(cmd.as_bytes());
            core::str::from_utf8(&buf[..prefix.len() + cmd.len()]).unwrap_or(cmd)
        };
        spawn(path, &tokens1[..count1])
    } else {
        -1
    };

    // Close write end in shell and restore stdout
    let _ = close(pipe_fds[1] as usize);
    let _ = dup2(saved_stdout, 1);
    let _ = close(saved_stdout);

    // Redirect stdin -> pipe read end
    let _ = dup2(pipe_fds[0] as usize, 0);
    let mut tokens2 = [""; 16];
    let count2 = split_words(cmd2, &mut tokens2);
    let pid2 = if count2 > 0 {
        let cmd = tokens2[0];
        let mut buf = [0u8; 64];
        let path = if cmd.starts_with('/') {
            cmd
        } else {
            let prefix = b"/bin/";
            buf[..prefix.len()].copy_from_slice(prefix);
            buf[prefix.len()..prefix.len() + cmd.len()].copy_from_slice(cmd.as_bytes());
            core::str::from_utf8(&buf[..prefix.len() + cmd.len()]).unwrap_or(cmd)
        };
        spawn(path, &tokens2[..count2])
    } else {
        -1
    };

    // Close read end in shell and restore stdin
    let _ = close(pipe_fds[0] as usize);
    let _ = dup2(saved_stdin, 0);
    let _ = close(saved_stdin);

    if pid1 > 0 {
        let _ = wait(pid1 as usize);
    }
    if pid2 > 0 {
        let _ = wait(pid2 as usize);
    }
}

fn execute_redirect(cmd: &str, out_file: &str) {
    let fd = open(out_file, 1);
    if fd < 0 {
        eprintln!("failed to open {} for writing", out_file);
        return;
    }
    let saved_stdout = 10;
    let _ = dup2(1, saved_stdout);
    let _ = dup2(fd as usize, 1);
    let _ = close(fd as usize);

    let mut tokens = [""; 16];
    let count = split_words(cmd, &mut tokens);
    if count > 0 {
        let c = tokens[0];
        let mut buf = [0u8; 64];
        let path = if c.starts_with('/') {
            c
        } else {
            let prefix = b"/bin/";
            buf[..prefix.len()].copy_from_slice(prefix);
            buf[prefix.len()..prefix.len() + c.len()].copy_from_slice(c.as_bytes());
            core::str::from_utf8(&buf[..prefix.len() + c.len()]).unwrap_or(c)
        };
        let pid = spawn(path, &tokens[..count]);
        if pid > 0 {
            let _ = wait(pid as usize);
        } else {
            eprintln!("{}: command not found", c);
        }
    }

    let _ = dup2(saved_stdout, 1);
    let _ = close(saved_stdout);
}

fn main(_args: &[&str]) -> i32 {
    println!("TriOS Shell v1.0 (Ring 3 Userland)");
    println!("Type 'exit' to quit.\n");

    let mut cwd_buf = [0u8; 128];
    let mut line_buf = [0u8; 256];

    loop {
        let cwd_len = getcwd(&mut cwd_buf);
        let cwd_str = if cwd_len > 0 {
            core::str::from_utf8(&cwd_buf[..cwd_len as usize]).unwrap_or("/")
        } else {
            "/"
        };

        print!("trios:{}$ ", cwd_str);

        let n = read_line(&mut line_buf);
        if n == 0 {
            continue;
        }

        if let Ok(line) = core::str::from_utf8(&line_buf[..n]) {
            execute_command(line);
        }
    }
}
