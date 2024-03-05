use crate::{sbi_call, EID_DBCN};

#[inline]
pub fn debug_console_write(
    num_bytes: usize,
    base_addr_lo: usize,
    base_addr_hi: usize,
) -> super::Result<usize> {
    let bytes_written =
        sbi_call!(ext: EID_DBCN, func: 0, "a0": num_bytes, "a1": base_addr_lo, "a2": base_addr_hi)?;

    Ok(bytes_written)
}
