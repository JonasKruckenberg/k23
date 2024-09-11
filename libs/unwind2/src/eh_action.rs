use crate::frame::Frame;
use crate::utils::deref_pointer;
use gimli::{constants, EndianSlice, NativeEndian, Pointer, Reader};

#[derive(Debug)]
pub enum EHAction {
    None,
    Cleanup(u64),
    Catch(u64),
}

pub fn find_eh_action(
    reader: &mut EndianSlice<'static, NativeEndian>,
    frame: &Frame<'_>,
) -> crate::Result<EHAction> {
    let func_start = frame.region_start();
    let ip = if frame.is_signal_trampoline() {
        frame.ip() as u64
    } else {
        frame.ip() as u64 - 1
    };

    let start_encoding = parse_pointer_encoding(reader)?;
    let lpad_base = if start_encoding.is_absent() {
        func_start
    } else {
        unsafe { deref_pointer(parse_encoded_pointer(start_encoding, frame, reader)?) }
    };

    let ttype_encoding = parse_pointer_encoding(reader)?;
    if !ttype_encoding.is_absent() {
        reader.read_uleb128()?;
    }

    let call_site_encoding = parse_pointer_encoding(reader)?;
    let call_site_table_length = reader.read_uleb128()?;
    reader.truncate(call_site_table_length.try_into().unwrap())?;

    while !reader.is_empty() {
        let cs_start =
            unsafe { deref_pointer(parse_encoded_pointer(call_site_encoding, frame, reader)?) };
        let cs_len =
            unsafe { deref_pointer(parse_encoded_pointer(call_site_encoding, frame, reader)?) };
        let cs_lpad =
            unsafe { deref_pointer(parse_encoded_pointer(call_site_encoding, frame, reader)?) };
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
        Err(gimli::Error::UnknownPointerEncoding(eh_pe))
    }
}

fn parse_encoded_pointer(
    encoding: constants::DwEhPe,
    frame: &Frame,
    input: &mut EndianSlice<'static, NativeEndian>,
) -> gimli::Result<Pointer> {
    if encoding == constants::DW_EH_PE_omit {
        return Err(gimli::Error::CannotParseOmitPointerEncoding);
    }

    let base = match encoding.application() {
        constants::DW_EH_PE_absptr => 0,
        constants::DW_EH_PE_pcrel => input.slice().as_ptr() as u64,
        constants::DW_EH_PE_textrel => frame.text_rel_base().unwrap_or(0),
        constants::DW_EH_PE_datarel => frame.data_rel_base().unwrap_or(0),
        constants::DW_EH_PE_funcrel => frame.region_start(),
        constants::DW_EH_PE_aligned => return Err(gimli::Error::UnsupportedPointerEncoding),
        _ => unreachable!(),
    };

    let offset = match encoding.format() {
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
    }?;

    let address = base.wrapping_add(offset);
    Ok(if encoding.is_indirect() {
        Pointer::Indirect(address)
    } else {
        Pointer::Direct(address)
    })
}
