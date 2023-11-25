use crate::{sbi_call, EID_RFENCE};

pub fn sfence_vma(
    hart_mask: usize,
    hart_mask_base: usize,
    start_addr: usize,
    size: usize,
) -> super::Result<()> {
    sbi_call(EID_RFENCE, 1, hart_mask, hart_mask_base, start_addr, size)?;
    Ok(())
}
