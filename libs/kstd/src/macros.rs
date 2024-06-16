#[macro_export]
macro_rules! hprint {
    ($s:expr) => {
        $crate::arch::hio::_print(format_args!($s))
    };
    ($($tt:tt)*) => {
        $crate::arch::hio::_print(format_args!($($tt)*))
    };
}

#[macro_export]
macro_rules! hprintln {
    () => {
        $crate::arch::hio::_print(format_args!("\n"))
    };
    ($s:expr) => {
        $crate::arch::hio::_print(format_args!(concat!($s, "\n")))
    };
    ($s:expr, $($tt:tt)*) => {
        $crate::arch::hio::_print(format_args!(concat!($s, "\n"), $($tt)*))
    };
}

#[macro_export]
macro_rules! heprint {
    ($s:expr) => {
        $crate::arch::hio::_eprint(format_args!($s))
    };
    ($($tt:tt)*) => {
        $crate::arch::hio::_eprint(format_args!($($tt)*))
    };
}

#[macro_export]
macro_rules! heprintln {
    () => {
        $crate::arch::hio::_eprint(format_args!("\n"))
    };
    ($s:expr) => {
        $crate::arch::hio::_eprint(format_args!(concat!($s, "\n")))
    };
    ($s:expr, $($tt:tt)*) => {
        $crate::arch::hio::_eprint(format_args!(concat!($s, "\n"), $($tt)*))
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
