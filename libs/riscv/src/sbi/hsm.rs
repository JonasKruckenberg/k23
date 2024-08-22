//! Hart State Management Extension

use super::{sbi_call, EID_HSM};

/// # Errors
///
/// Returns an error if the SBI call fails.
#[inline]
pub fn start_hart(hartid: usize, start_address: usize, opaque: usize) -> super::Result<()> {
    sbi_call!(ext: EID_HSM, func: 0, "a0": hartid, "a1": start_address, "a2": opaque)?;
    Ok(())
}
