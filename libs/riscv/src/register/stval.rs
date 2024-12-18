//! Supervisor Trap Value Register

use super::{read_csr_as_usize, write_csr_as_usize};

read_csr_as_usize!(0x143);
write_csr_as_usize!(0x143);
