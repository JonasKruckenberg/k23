// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

cfg_if::cfg_if! {
    if #[cfg(target_pointer_width = "64")] {
        #[macro_export]
        macro_rules! x {
            ($val32:expr, $val64:expr) => {
                $val64
            };
        }
    } else if #[cfg(target_pointer_width = "32")] {
        #[macro_export]
        macro_rules! x {
            ($val32:expr, $val64:expr) => {
                $val32
            };
        }
    }
}

#[macro_export]
macro_rules! xlen_bytes {
    () => {
        $crate::x!("4", "8")
    };
    ($word_offset:expr) => {
        concat!(
            "((",
            stringify!($word_offset),
            ") * ",
            $crate::xlen_bytes!(),
            ")"
        )
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! define_op {
    ($ins:literal, $reg:ident, $pos:expr, $ptr:ident) => {
        concat!(
            $ins,
            " ",
            stringify!($reg),
            ", ",
            $crate::xlen_bytes!(),
            "*",
            $pos,
            '(',
            stringify!($ptr),
            ')'
        )
    };
}

#[macro_export]
macro_rules! load_gp {
    ($ptr:ident[$pos:expr] => $reg:ident) => {
        $crate::define_op!("ld", $reg, $pos, $ptr)
    };
}

#[macro_export]
macro_rules! save_gp {
    ($reg:ident => $ptr:ident[$pos:expr]) => {
        $crate::define_op!("sd", $reg, $pos, $ptr)
    };
}

#[macro_export]
macro_rules! load_fp {
    ($ptr:ident[$pos:expr] => $reg:ident) => {
        $crate::define_op!("fld", $reg, $pos, $ptr)
    };
}

#[macro_export]
macro_rules! save_fp {
    ($reg:ident => $ptr:ident[$pos:expr]) => {
        $crate::define_op!("fsd", $reg, $pos, $ptr)
    };
}
