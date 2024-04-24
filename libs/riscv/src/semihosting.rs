use core::arch::asm;
use core::fmt::{Error, Write};
use core::ops::DerefMut;
use core::{fmt, slice};
use sync::Mutex;

const OPEN: usize = 0x01;
const WRITE: usize = 0x05;
const OPEN_W_TRUNC: usize = 4;

struct HostStream(usize);

impl HostStream {
    pub fn new_stdout() -> Self {
        Self::open(":tt\0", OPEN_W_TRUNC).unwrap()
    }

    pub fn new_stderr() -> Self {
        Self::open(":tt\0", OPEN_W_TRUNC).unwrap()
    }

    fn open(name: &str, mode: usize) -> Result<Self, ()> {
        let name = name.as_bytes();
        match unsafe { syscall(OPEN, &[name.as_ptr() as usize, mode, name.len() - 1]) } as isize {
            -1 => Err(()),
            fd => Ok(Self(fd as usize)),
        }
    }
}

impl Write for HostStream {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut buf = s.as_bytes();
        while !buf.is_empty() {
            match unsafe { syscall(WRITE, &[self.0, buf.as_ptr() as usize, buf.len()]) } {
                // Done
                0 => return Ok(()),
                // `n` bytes were not written
                n if n <= buf.len() => {
                    let offset = (buf.len() - n) as isize;
                    buf = unsafe { slice::from_raw_parts(buf.as_ptr().offset(offset), n) }
                }
                // Error
                _ => return Err(Error::default()),
            }
        }
        Ok(())
    }
}

unsafe fn syscall(_nr: usize, _arg: &[usize]) -> usize {
    cfg_if::cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            let mut nr = _nr;
            let arg = _arg;
            // The instructions below must always be uncompressed, otherwise
            // it will be treated as a regular break, hence the norvc option.
            //
            // See https://github.com/riscv/riscv-semihosting-spec for more details.
            asm!("
                .balign 16
                .option push
                .option norvc
                slli x0, x0, 0x1f
                ebreak
                srai x0, x0, 0x7
                .option pop
                ",
                inout("a0") nr,
                inout("a1") arg.as_ptr() => _,
                options(nostack, preserves_flags),
            );
            nr
        } else {
            unimplemented!();
        }
    }
}

static STDOUT: Mutex<Option<HostStream>> = Mutex::new(None);
static STDERR: Mutex<Option<HostStream>> = Mutex::new(None);

fn print_inner(hs: &'static Mutex<Option<HostStream>>, args: fmt::Arguments) -> fmt::Result {
    let mut stream = hs.lock();

    if stream.is_none() {
        stream.replace(HostStream::new_stdout());
    }

    match stream.deref_mut() {
        Some(stream) => stream.write_fmt(args),
        None => unreachable!(),
    }
}

pub fn _print(args: fmt::Arguments) {
    print_inner(&STDOUT, args).expect("failed to write to semihosting stdout")
}
pub fn _eprint(args: fmt::Arguments) {
    print_inner(&STDERR, args).expect("failed to write to semihosting stderr")
}

#[macro_export]
macro_rules! hprint {
    ($s:expr) => {
        $crate::semihosting::_print(format_args!($s))
    };
    ($($tt:tt)*) => {
        $crate::semihosting::_print(format_args!($($tt)*))
    };
}

#[macro_export]
macro_rules! hprintln {
    () => {
        $crate::semihosting::_print(format_args!("\n"))
    };
    ($s:expr) => {
        $crate::semihosting::_print(format_args!(concat!($s, "\n")))
    };
    ($s:expr, $($tt:tt)*) => {
        $crate::semihosting::_print(format_args!(concat!($s, "\n"), $($tt)*))
    };
}

#[macro_export]
macro_rules! heprint {
    ($s:expr) => {
        $crate::semihosting::_eprint(format_args!($s))
    };
    ($($tt:tt)*) => {
        $crate::semihosting::_eprint(format_args!($($tt)*))
    };
}

#[macro_export]
macro_rules! heprintln {
    () => {
        $crate::semihosting::_eprint(format_args!("\n"))
    };
    ($s:expr) => {
        $crate::semihosting::_eprint(format_args!(concat!($s, "\n")))
    };
    ($s:expr, $($tt:tt)*) => {
        $crate::semihosting::_eprint(format_args!(concat!($s, "\n"), $($tt)*))
    };
}

#[macro_export]
macro_rules! dbg {
    () => {
        $crate::heprintln!("[{}:{}]", file!(), line!());
    };
    ($val:expr) => {
        // Use of `match` here is intentional because it affects the lifetimes
        // of temporaries - https://stackoverflow.com/a/48732525/1063961
        match $val {
            tmp => {
                $crate::heprintln!("[{}:{}] {} = {:#?}",
                    file!(), line!(), stringify!($val), &tmp);
                tmp
            }
        }
    };
    // Trailing comma with single argument is ignored
    ($val:expr,) => { $crate::dbg!($val) };
    ($($val:expr),+ $(,)?) => {
        ($($crate::dbg!($val)),+,)
    };
}

pub use {dbg, heprint, heprintln, hprint, hprintln};
