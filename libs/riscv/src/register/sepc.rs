//! Supervisor Exception Program Counter Register

use super::{read_csr_as_usize, set_csr_as_usize};

read_csr_as_usize!(0x141);
set_csr_as_usize!(0x141);
