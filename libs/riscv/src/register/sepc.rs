//! Supervisor Exception Program Counter Register

use super::{read_csr_as_usize, write_csr_as_usize};

read_csr_as_usize!(0x141);
write_csr_as_usize!(0x141);
