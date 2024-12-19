macro_rules! panic_in_drop {
    ($($arg:tt)*) => {
        if !panic_unwind::panicking() {
            panic!($($arg)*)
        } else {
            log::error!(
                "thread attempted to panic at '{msg}', {file}:{line}:{col}\n\
                note: we were already unwinding due to a previous panic.",
                msg = format_args!($($arg)*),
                file = file!(),
                line = line!(),
                col = column!(),
            );
        }
    }
}

macro_rules! debug_assert_eq_in_drop {
    ($this:expr, $that:expr) => {
        debug_assert_eq_in_drop!(@inner $this, $that, "")
    };
    ($this:expr, $that:expr, $($arg:tt)+) => {
        debug_assert_eq_in_drop!(@inner $this, $that, format_args!(": {}", format_args!($($arg)+)))
    };
    (@inner $this:expr, $that:expr, $msg:expr) => {
        if cfg!(debug_assertions) {
            if $this != $that {
                panic_in_drop!(
                    "assertion failed ({} == {})\n  left: `{:?}`,\n right: `{:?}`{}",
                    stringify!($this),
                    stringify!($that),
                    $this,
                    $that,
                    $msg,
                )
            }
        }
    }
}
