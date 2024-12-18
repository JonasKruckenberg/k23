//! Time Register

use super::{read_composite_csr, read_csr_as_usize};

read_csr_as_usize!(0xC01);
read_composite_csr!(super::timeh::read(), read());
