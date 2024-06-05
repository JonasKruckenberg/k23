use super::{sbi_call, EID_BASE};

pub struct SbiVersion {
    pub minor: usize,
    pub major: usize,
}

/// Returns the current SBI specification version.
#[inline]
pub fn get_spec_version() -> super::Result<SbiVersion> {
    let version = sbi_call!(ext: EID_BASE, func: 0)?;

    Ok(SbiVersion {
        minor: version & 0xffffff,
        major: (version & 0x7f000000) >> 24,
    })
}

/// Returns the current SBI implementation ID, which is different for every SBI implementation.
///
/// It is intended that this implementation ID allows software to probe for SBI implementation quirks.
///
/// # Known Implementation IDs
///
/// | Implementation ID | Name                              |
/// |-------------------|-----------------------------------|
/// | 0                 | Berkeley Boot Loader (BBL)        |
/// | 1                 | OpenSBI                           |
/// | 2                 | Xvisor                            |
/// | 3                 | KVM                               |
/// | 4                 | RustSBI                           |
/// | 5                 | Diosix                            |
/// | 6                 | Coffer                            |
/// | 7                 | Xen Project                       |
/// | 8                 | PolarFire Hart Software Services  |
#[inline]
pub fn get_impl_id() -> super::Result<usize> {
    let id = sbi_call!(ext: EID_BASE, func: 1)?;

    Ok(id)
}

/// Returns the current SBI implementation version.
///
/// The encoding of this version number is specific to the SBI implementation.
#[inline]
pub fn get_impl_version() -> super::Result<usize> {
    let id = sbi_call!(ext: EID_BASE, func: 2)?;

    Ok(id)
}

/// Returns whether the given SBI extension ID (EID) is available.
#[inline]
pub fn probe_sbi_extension(ext: usize) -> super::Result<bool> {
    let id = sbi_call!(ext: EID_BASE, func: 3, "a0": ext)?;

    Ok(id == 1)
}

/// Return a value that is legal for the `mvendorid` CSR.
#[inline]
pub fn get_machine_vendor_id() -> super::Result<usize> {
    let id = sbi_call!(ext: EID_BASE, func: 4)?;

    Ok(id)
}

/// Return a value that is legal for the `marchid` CSR
#[inline]
pub fn get_machine_architecture_id() -> super::Result<usize> {
    let id = sbi_call!(ext: EID_BASE, func: 5)?;

    Ok(id)
}

/// Return a value that is legal for the `mimpid` CSR
#[inline]
pub fn get_machine_impl_id() -> super::Result<usize> {
    let id = sbi_call!(ext: EID_BASE, func: 6)?;

    Ok(id)
}