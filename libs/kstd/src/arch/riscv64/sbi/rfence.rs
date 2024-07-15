use super::{sbi_call, EID_RFENCE};

/// # Errors
///
/// Returns an error if the SBI call fails.
#[inline]
pub fn sfence_vma(
    hart_mask: usize,
    hart_mask_base: usize,
    start_addr: usize,
    size: usize,
) -> super::Result<()> {
    sbi_call!(ext: EID_RFENCE, func: 2, "a0": hart_mask, "a1": hart_mask_base, "a2": start_addr, "a3": size)?;
    Ok(())
}

/// # Errors
///
/// Returns an error if the SBI call fails.
#[inline]
pub fn sfence_vma_asid(
    hart_mask: usize,
    hart_mask_base: usize,
    start_addr: usize,
    size: usize,
    asid: usize,
) -> super::Result<()> {
    sbi_call!(ext: EID_RFENCE, func: 2, "a0": hart_mask, "a1": hart_mask_base, "a2": start_addr, "a3": size, "a4": asid)?;
    Ok(())
}
