// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

/// Helper macro for constructing the inline assembly, used below.
macro_rules! define_op {
    ($ins:literal, $reg:ident, $ptr_width:literal, $pos:expr, $ptr:ident) => {
        concat!(
            $ins,
            " ",
            stringify!($reg),
            ", ",
            stringify!($ptr_width),
            "*",
            $pos,
            '(',
            stringify!($ptr),
            ')'
        )
    };
}

cfg_if::cfg_if! {
    if #[cfg(target_pointer_width = "32")] {
        macro_rules! save_gp {
            ($reg:ident => $ptr:ident[$pos:expr]) => {
                define_op!("sw", $reg, 4, $pos, $ptr)
            }
        }
        macro_rules! load_gp {
            ($ptr:ident[$pos:expr] => $reg:ident) => {
                define_op!("lw", $reg, 4, $pos, $ptr)
            }
        }
        macro_rules! save_fp {
            ($reg:ident => $ptr:ident[$pos:expr]) => {
                define_op!("fsw", $reg, 4, $pos, $ptr)
            }
        }
        macro_rules! load_fp {
            ($ptr:ident[$pos:expr] => $reg:ident) => {
                define_op!("flw", $reg, 4, $pos, $ptr)
            }
        }
    } else if #[cfg(target_pointer_width = "64")] {
        macro_rules! load_gp {
            ($ptr:ident[$pos:expr] => $reg:ident) => {
                define_op!("ld", $reg, 8, $pos, $ptr)
            }
        }
        macro_rules! save_gp {
            ($reg:ident => $ptr:ident[$pos:expr]) => {
                define_op!("sd", $reg, 8, $pos, $ptr)
            }
        }
        macro_rules! load_fp {
            ($ptr:ident[$pos:expr] => $reg:ident) => {
                define_op!("fld", $reg, 8, $pos, $ptr)
            }
        }
        macro_rules! save_fp {
            ($reg:ident => $ptr:ident[$pos:expr]) => {
                define_op!("fsd", $reg, 8, $pos, $ptr)
            }
        }
    }
}

pub(crate) use {define_op, load_fp, load_gp, save_fp, save_gp};
