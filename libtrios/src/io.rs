//! Userland standard I/O macros.

use core::fmt::{Arguments, Result, Write};
use crate::write;

struct FdWriter(usize);

impl Write for FdWriter {
    fn write_str(&mut self, s: &str) -> Result {
        write(self.0, s.as_bytes());
        Ok(())
    }
}

pub fn _print(args: Arguments) {
    let mut writer = FdWriter(1);
    let _ = writer.write_fmt(args);
}

pub fn _eprint(args: Arguments) {
    let mut writer = FdWriter(2);
    let _ = writer.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::io::_print(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => {{
        $crate::io::_print(core::format_args!($($arg)*));
        $crate::print!("\n");
    }};
}

#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => {
        $crate::io::_eprint(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! eprintln {
    () => ($crate::eprint!("\n"));
    ($($arg:tt)*) => {{
        $crate::io::_eprint(core::format_args!($($arg)*));
        $crate::eprint!("\n");
    }};
}
