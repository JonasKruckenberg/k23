#![no_std]
#![feature(thread_local, error_in_core)]

pub mod arch;
pub mod sync;

#[macro_export]
macro_rules! print {
    ($s:expr) => {
        $crate::arch::riscv64::semihosting::_print(format_args!($s))
    };
    ($($tt:tt)*) => {
        $crate::arch::riscv64::semihosting::_print(format_args!($($tt)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::arch::riscv64::semihosting::_print(format_args!("\n"))
    };
    ($s:expr) => {
        $crate::arch::riscv64::semihosting::_print(format_args!(concat!($s, "\n")))
    };
    ($s:expr, $($tt:tt)*) => {
        $crate::arch::riscv64::semihosting::_print(format_args!(concat!($s, "\n"), $($tt)*))
    };
}

#[macro_export]
macro_rules! eprint {
    ($s:expr) => {
        $crate::arch::riscv64::semihosting::_eprint(format_args!($s))
    };
    ($($tt:tt)*) => {
        $crate::arch::riscv64::semihosting::_eprint(format_args!($($tt)*))
    };
}

#[macro_export]
macro_rules! eprintln {
    () => {
        $crate::arch::riscv64::semihosting::_eprint(format_args!("\n"))
    };
    ($s:expr) => {
        $crate::arch::riscv64::semihosting::_eprint(format_args!(concat!($s, "\n")))
    };
    ($s:expr, $($tt:tt)*) => {
        $crate::arch::riscv64::semihosting::_eprint(format_args!(concat!($s, "\n"), $($tt)*))
    };
}

#[macro_export]
macro_rules! dbg {
    () => {
        $crate::arch::heprintln!("[{}:{}]", file!(), line!());
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
