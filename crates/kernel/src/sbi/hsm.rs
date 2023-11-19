use crate::sbi::{sbi_call, EID_HSM};

#[inline]
pub fn start_hart(hartid: usize, start_address: usize, opaque: usize) -> super::Result<()> {
    sbi_call(EID_HSM, 0, hartid, start_address, opaque)?;
    Ok(())
}
