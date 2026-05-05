mod arch;
mod machine;
mod memory;
pub mod proptest;

pub use arch::EmulateArch;
pub use machine::{Cpu, HasMemory, Machine, MachineBuilder, MissingMemory};
pub use memory::Memory;

#[macro_export]
macro_rules! for_every_arch {
    ($arch:ident => {$($body:item)*}) => {
        mod riscv64_sv39 {
            use super::*;
            type $arch = $crate::arch::riscv64::Riscv64Sv39;

            $($body)*
        }
        mod riscv64_sv48 {
            use super::*;
            type $arch = $crate::arch::riscv64::Riscv64Sv48;

            $($body)*
        }
        mod riscv64_sv57 {
            use super::*;
            type $arch = $crate::arch::riscv64::Riscv64Sv57;

            $($body)*
        }
    };
}

#[macro_export]
macro_rules! archtest {
    ($($(#[$meta:meta])* fn $test_name:ident$(<$ge:ident: $gen_ty:tt>)*() $body:block)*) => {
        mod riscv64_sv39 {
            use super::*;
            $(
                archtest! {
                    arch: $crate::arch::riscv64::Riscv64Sv39,
                    meta: $($meta)*,
                    test_name: $test_name,
                    generics: $($ge: $gen_ty)*,
                    body: $body
                }
            )*
        }
        mod riscv64_sv48 {
            use super::*;
            $(
                archtest! {
                    arch: $crate::arch::riscv64::Riscv64Sv48,
                    meta: $($meta)*,
                    test_name: $test_name,
                    generics: $($ge: $gen_ty)*,
                    body: $body
                }
            )*
        }
        mod riscv64_sv57 {
            use super::*;
            $(
                archtest! {
                    arch: $crate::arch::riscv64::Riscv64Sv57,
                    meta: $($meta)*,
                    test_name: $test_name,
                    generics: $($ge: $gen_ty)*,
                    body: $body
                }
            )*
        }
    };

    (arch: $arch:ty, meta: $($($meta:meta)*, test_name: $test_name:ident, generics: $($ge:ident: $gen_ty:tt)*, body: $body:block)*) => {
        $(
            $(#[$meta])*
            fn $test_name() {
                fn $test_name$(<$ge: $gen_ty>)*() $body

                $test_name::<$arch>()
            }
        )*
    }
}
