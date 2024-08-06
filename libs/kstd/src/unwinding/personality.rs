#![allow(clippy::cast_sign_loss)]

use super::{
    utils::{deref_pointer, get_unlimited_slice},
    UnwindAction, UnwindContext, UnwindException, UnwindReasonCode,
};
use crate::arch;
use core::{ffi::c_int, mem};
use gimli::{constants, EndianSlice, NativeEndian, Pointer, Reader};

#[lang = "eh_personality"]
unsafe fn rust_eh_personality(
    version: c_int,
    actions: UnwindAction,
    _exception_class: u64,
    exception: *mut UnwindException,
    unwind_ctx: &mut UnwindContext<'_>,
) -> UnwindReasonCode {
    if version != 1 {
        return UnwindReasonCode::FATAL_PHASE1_ERROR;
    }

    let lsda = crate::unwinding::_Unwind_GetLanguageSpecificData(unwind_ctx);
    if lsda.is_null() {
        return UnwindReasonCode::CONTINUE_UNWIND;
    }

    let mut lsda = EndianSlice::new(unsafe { get_unlimited_slice(lsda as _) }, NativeEndian);
    let Ok(eh_action) = find_eh_action(&mut lsda, unwind_ctx) else {
        return UnwindReasonCode::FATAL_PHASE1_ERROR;
    };

    if actions.contains(UnwindAction::SEARCH_PHASE) {
        match eh_action {
            EHAction::None | EHAction::Cleanup(_) => UnwindReasonCode::CONTINUE_UNWIND,
            EHAction::Catch(_) => UnwindReasonCode::HANDLER_FOUND,
        }
    } else {
        match eh_action {
            EHAction::None => UnwindReasonCode::CONTINUE_UNWIND,
            EHAction::Cleanup(lpad) | EHAction::Catch(lpad) => {
                crate::unwinding::_Unwind_SetGR(
                    unwind_ctx,
                    i32::from(arch::unwinding::UNWIND_DATA_REG.0 .0),
                    exception as usize,
                );
                crate::unwinding::_Unwind_SetGR(
                    unwind_ctx,
                    i32::from(arch::unwinding::UNWIND_DATA_REG.1 .0),
                    0,
                );
                crate::unwinding::_Unwind_SetIP(unwind_ctx, lpad);
                UnwindReasonCode::INSTALL_CONTEXT
            }
        }
    }
}

#[derive(Debug)]
enum EHAction {
    None,
    Cleanup(usize),
    Catch(usize),
}

fn find_eh_action(
    reader: &mut EndianSlice<'static, NativeEndian>,
    unwind_ctx: &UnwindContext<'_>,
) -> gimli::Result<EHAction> {
    let func_start = crate::unwinding::_Unwind_GetRegionStart(unwind_ctx);
    let mut ip_before_instr = 0;
    let ip = crate::unwinding::_Unwind_GetIPInfo(unwind_ctx, &mut ip_before_instr);
    let ip = if ip_before_instr != 0 { ip } else { ip - 1 };

    let start_encoding = parse_pointer_encoding(reader)?;
    let lpad_base = if start_encoding.is_absent() {
        func_start
    } else {
        unsafe { deref_pointer(parse_encoded_pointer(start_encoding, unwind_ctx, reader)?) }
    };

    let ttype_encoding = parse_pointer_encoding(reader)?;
    if !ttype_encoding.is_absent() {
        reader.read_uleb128()?;
    }

    let call_site_encoding = parse_pointer_encoding(reader)?;
    let call_site_table_length = reader.read_uleb128()?;
    reader.truncate(call_site_table_length.try_into().unwrap())?;

    while !reader.is_empty() {
        let cs_start = unsafe {
            deref_pointer(parse_encoded_pointer(
                call_site_encoding,
                unwind_ctx,
                reader,
            )?)
        };
        let cs_len = unsafe {
            deref_pointer(parse_encoded_pointer(
                call_site_encoding,
                unwind_ctx,
                reader,
            )?)
        };
        let cs_lpad = unsafe {
            deref_pointer(parse_encoded_pointer(
                call_site_encoding,
                unwind_ctx,
                reader,
            )?)
        };
        let cs_action = reader.read_uleb128()?;
        if ip < func_start + cs_start {
            break;
        }
        if ip < func_start + cs_start + cs_len {
            return if cs_lpad == 0 {
                Ok(EHAction::None)
            } else {
                let lpad = lpad_base + cs_lpad;
                Ok(match cs_action {
                    0 => EHAction::Cleanup(lpad),
                    _ => EHAction::Catch(lpad),
                })
            };
        }
    }
    Ok(EHAction::None)
}

fn parse_pointer_encoding(
    input: &mut EndianSlice<'static, NativeEndian>,
) -> gimli::Result<constants::DwEhPe> {
    let eh_pe = input.read_u8()?;
    let eh_pe = constants::DwEhPe(eh_pe);

    if eh_pe.is_valid_encoding() {
        Ok(eh_pe)
    } else {
        Err(gimli::Error::UnknownPointerEncoding)
    }
}

fn parse_encoded_pointer(
    encoding: constants::DwEhPe,
    unwind_ctx: &UnwindContext<'_>,
    input: &mut EndianSlice<'static, NativeEndian>,
) -> gimli::Result<Pointer> {
    if encoding == constants::DW_EH_PE_omit {
        return Err(gimli::Error::CannotParseOmitPointerEncoding);
    }

    let base = match encoding.application() {
        constants::DW_EH_PE_absptr => 0,
        constants::DW_EH_PE_pcrel => input.slice().as_ptr() as u64,
        constants::DW_EH_PE_textrel => crate::unwinding::_Unwind_GetTextRelBase(unwind_ctx) as u64,
        constants::DW_EH_PE_datarel => crate::unwinding::_Unwind_GetDataRelBase(unwind_ctx) as u64,
        constants::DW_EH_PE_funcrel => crate::unwinding::_Unwind_GetRegionStart(unwind_ctx) as u64,
        constants::DW_EH_PE_aligned => return Err(gimli::Error::UnsupportedPointerEncoding),
        _ => unreachable!(),
    };

    let offset = match encoding.format() {
        constants::DW_EH_PE_absptr => {
            input.read_address(mem::size_of::<usize>().try_into().unwrap())
        }
        constants::DW_EH_PE_uleb128 => input.read_uleb128(),
        constants::DW_EH_PE_udata2 => input.read_u16().map(u64::from),
        constants::DW_EH_PE_udata4 => input.read_u32().map(u64::from),
        constants::DW_EH_PE_udata8 => input.read_u64(),
        constants::DW_EH_PE_sleb128 => input.read_sleb128().map(|a| a as u64),
        constants::DW_EH_PE_sdata2 => input.read_i16().map(|a| a as u64),
        constants::DW_EH_PE_sdata4 => input.read_i32().map(|a| a as u64),
        constants::DW_EH_PE_sdata8 => input.read_i64().map(|a| a as u64),
        _ => unreachable!(),
    }?;

    let address = base.wrapping_add(offset);
    Ok(if encoding.is_indirect() {
        Pointer::Indirect(address)
    } else {
        Pointer::Direct(address)
    })
}
