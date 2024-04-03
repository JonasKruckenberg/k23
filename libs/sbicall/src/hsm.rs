use crate::{sbi_call, EID_HSM};

#[inline]
pub fn start_hart(hartid: usize, start_address: usize, opaque: usize) -> super::Result<()> {
    sbi_call!(ext: EID_HSM, func: 0, "a0": hartid, "a1": start_address, "a2": opaque)?;
    Ok(())
}
