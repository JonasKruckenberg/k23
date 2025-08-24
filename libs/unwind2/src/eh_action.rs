// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! This module implements parsing of GCC-style Language-Specific Data Area (LSDA)
//! and determining the appropriate `EhAction` for a given IP.
//!
//! For details see:
//!  * <https://refspecs.linuxfoundation.org/LSB_3.0.0/LSB-PDA/LSB-PDA/ehframechpt.html>
//!  * <https://refspecs.linuxfoundation.org/LSB_5.0.0/LSB-Core-generic/LSB-Core-generic/dwarfext.html>
//!  * <https://itanium-cxx-abi.github.io/cxx-abi/exceptions.pdf>
//!  * <https://www.airs.com/blog/archives/460>
//!  * <https://www.airs.com/blog/archives/464>
//!
//! A reference implementation may be found in the GCC source tree
//! (`<root>/libgcc/unwind-c.c` as of this writing).

use gimli::{EndianSlice, NativeEndian, Pointer, Reader, constants};

use crate::frame::Frame;
use crate::utils::deref_pointer;

type LPad = *const u8;
#[derive(Debug)]
pub enum EHAction {
    None,
    Cleanup(LPad),
    Catch(LPad),
    Filter(LPad),
    Terminate,
}

pub fn find_eh_action(
    reader: &mut EndianSlice<'static, NativeEndian>,
    frame: &Frame<'_>,
) -> crate::Result<EHAction> {
    let func_start = frame.symbol_address();
    let ip = if frame.is_signal_trampoline() {
        frame.ip() as u64
    } else {
        frame.ip() as u64 - 1
    };

    let start_encoding = parse_pointer_encoding(reader)?;
    let lpad_base = if start_encoding.is_absent() {
        func_start as *const u8
    } else {
        // Safety: We have to trust the DWARF info here
        read_encoded_pointer(reader, start_encoding, frame)?
    };

    let ttype_encoding = parse_pointer_encoding(reader)?;
    if !ttype_encoding.is_absent() {
        reader.read_uleb128()?;
    }

    let call_site_encoding = parse_pointer_encoding(reader)?;
    let call_site_table_length = reader.read_uleb128()?;
    let (mut call_site_table, action_table) =
        reader.split_at(usize::try_from(call_site_table_length).unwrap());

    while !reader.is_empty() {
        // Safety: We have to trust the DWARF info here
        unsafe {
            // these are offsets rather than pointers;
            let cs_start = read_encoded_offset(&mut call_site_table, call_site_encoding)?;
            let cs_len = read_encoded_offset(&mut call_site_table, call_site_encoding)?;
            let cs_lpad = read_encoded_offset(&mut call_site_table, call_site_encoding)?;
            let cs_action_entry = call_site_table.read_uleb128()?;
            // Callsite table is sorted by cs_start, so if we've passed the ip, we
            // may stop searching.
            if ip < func_start.wrapping_add(cs_start) {
                break;
            }

            if ip < func_start.wrapping_add(cs_start + cs_len) {
                if cs_lpad == 0 {
                    return Ok(EHAction::None);
                } else {
                    let lpad = lpad_base.wrapping_add(usize::try_from(cs_lpad).unwrap());
                    return interpret_cs_action(action_table, cs_action_entry, lpad);
                }
            }
        }
    }
    // Ip is not present in the table. This indicates a nounwind call.
    Ok(EHAction::Terminate)
}

unsafe fn interpret_cs_action(
    mut action_table: EndianSlice<'_, NativeEndian>,
    cs_action_entry: u64,
    lpad: LPad,
) -> crate::Result<EHAction> {
    if cs_action_entry == 0 {
        // If cs_action_entry is 0 then this is a cleanup (Drop::drop). We run these
        // for both Rust panics and foreign exceptions.
        Ok(EHAction::Cleanup(lpad))
    } else {
        // If lpad != 0 and cs_action_entry != 0, we have to check ttype_index.
        // If ttype_index == 0 under the condition, we take cleanup action.
        action_table.skip(usize::try_from(cs_action_entry - 1).unwrap())?;
        let ttype_index = action_table.read_sleb128()?;
        if ttype_index == 0 {
            Ok(EHAction::Cleanup(lpad))
        } else if ttype_index > 0 {
            // Stop unwinding Rust panics at catch_unwind.
            Ok(EHAction::Catch(lpad))
        } else {
            Ok(EHAction::Filter(lpad))
        }
    }
}

fn parse_pointer_encoding(
    input: &mut EndianSlice<'static, NativeEndian>,
) -> gimli::Result<constants::DwEhPe> {
    let eh_pe = input.read_u8()?;
    let eh_pe = constants::DwEhPe(eh_pe);

    if eh_pe.is_valid_encoding() {
        Ok(eh_pe)
    } else {
        Err(gimli::Error::UnknownPointerEncoding(eh_pe))
    }
}

fn read_encoded_pointer(
    input: &mut EndianSlice<'static, NativeEndian>,
    encoding: constants::DwEhPe,
    frame: &Frame,
) -> gimli::Result<*const u8> {
    if encoding == constants::DW_EH_PE_omit {
        return Err(gimli::Error::CannotParseOmitPointerEncoding);
    }

    let base = match encoding.application() {
        constants::DW_EH_PE_absptr => input.read_address(size_of::<usize>().try_into().unwrap())?,
        // relative to address of the encoded value, despite the name
        constants::DW_EH_PE_pcrel => input.slice().as_ptr() as u64,
        constants::DW_EH_PE_funcrel => {
            if frame.symbol_address() == 0 {
                return Err(gimli::Error::UnsupportedPointerEncoding);
            }
            frame.symbol_address()
        }
        constants::DW_EH_PE_textrel => frame
            .text_rel_base()
            .ok_or(gimli::Error::UnsupportedPointerEncoding)?,
        constants::DW_EH_PE_datarel => frame
            .data_rel_base()
            .ok_or(gimli::Error::UnsupportedPointerEncoding)?,
        constants::DW_EH_PE_aligned => {
            return Err(gimli::Error::UnsupportedPointerEncoding);
        }
        _ => return Err(gimli::Error::UnsupportedPointerEncoding),
    };

    debug_assert_ne!(base, 0);

    let offset = read_encoded_offset(input, encoding)?;
    let address = base.wrapping_add(offset);

    let pointer = if encoding.is_indirect() {
        Pointer::Indirect(address)
    } else {
        Pointer::Direct(address)
    };

    // Safety: we have to trust the DWARF info here
    Ok(unsafe { deref_pointer(pointer) as *const u8 })
}

#[expect(
    clippy::cast_sign_loss,
    reason = "numeric casts are checked and behave as expected"
)]
fn read_encoded_offset(
    input: &mut EndianSlice<'static, NativeEndian>,
    encoding: constants::DwEhPe,
) -> gimli::Result<u64> {
    if encoding == constants::DW_EH_PE_omit {
        return Err(gimli::Error::CannotParseOmitPointerEncoding);
    }

    match encoding.format() {
        constants::DW_EH_PE_absptr => input.read_address(size_of::<usize>().try_into().unwrap()),
        constants::DW_EH_PE_uleb128 => input.read_uleb128(),
        constants::DW_EH_PE_udata2 => input.read_u16().map(u64::from),
        constants::DW_EH_PE_udata4 => input.read_u32().map(u64::from),
        constants::DW_EH_PE_udata8 => input.read_u64(),
        constants::DW_EH_PE_sleb128 => input.read_sleb128().map(|a| a as u64),
        constants::DW_EH_PE_sdata2 => input.read_i16().map(|a| a as u64),
        constants::DW_EH_PE_sdata4 => input.read_i32().map(|a| a as u64),
        constants::DW_EH_PE_sdata8 => input.read_i64().map(|a| a as u64),
        _ => unreachable!(),
    }
}
