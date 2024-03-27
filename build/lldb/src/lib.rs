mod sb;
mod strings;

pub use sb::_type::*;
pub use sb::address::*;
pub use sb::attach_info::*;
pub use sb::breakpoint::*;
pub use sb::breakpoint_location::*;
pub use sb::broadcaster::*;
pub use sb::command_interpreter::*;
pub use sb::command_return_object::*;
pub use sb::compile_unit::*;
pub use sb::data::*;
pub use sb::debugger::*;
pub use sb::error::*;
pub use sb::event::*;
pub use sb::execution_context::*;
pub use sb::file::*;
pub use sb::file_spec::*;
pub use sb::frame::*;
pub use sb::instruction::*;
pub use sb::instruction_list::*;
pub use sb::launch_info::*;
pub use sb::line_entry::*;
pub use sb::listener::*;
pub use sb::memory_region_info::*;
pub use sb::memory_region_info_list::*;
pub use sb::module::*;
pub use sb::module_spec::*;
pub use sb::platform::*;
pub use sb::process::*;
pub use sb::section::*;
pub use sb::stream::*;
pub use sb::string_list::*;
pub use sb::structured_data::*;
pub use sb::symbol::*;
pub use sb::symbol_context::*;
pub use sb::symbol_context_list::*;
pub use sb::target::*;
pub use sb::thread::*;
pub use sb::value::*;
pub use sb::value_list::*;
pub use sb::watchpoint::*;

pub type Address = u64;
pub type ProcessID = u64;
pub type ThreadID = u64;
pub type BreakpointID = u32;
pub type WatchpointID = u32;
pub type UserID = u64;

use std::{fmt, str};

fn debug_descr<CPP>(f: &mut fmt::Formatter, cpp: CPP) -> fmt::Result
where
    CPP: FnOnce(&mut SBStream) -> bool,
{
    let mut descr = SBStream::new();
    if cpp(&mut descr) {
        match str::from_utf8(descr.data()) {
            Ok(s) => f.write_str(s),
            Err(_) => Err(fmt::Error),
        }
    } else {
        Ok(())
    }
}

pub trait IsValid {
    fn is_valid(&self) -> bool;

    /// If `self.is_valid()` is `true`, returns `Some(self)`, otherwise `None`.
    fn check(self) -> Option<Self>
    where
        Self: Sized,
    {
        if self.is_valid() {
            Some(self)
        } else {
            None
        }
    }
}

struct SBIterator<Item, GetItem>
where
    GetItem: FnMut(u32) -> Item,
{
    size: u32,
    get_item: GetItem,
    index: u32,
}

impl<Item, GetItem> SBIterator<Item, GetItem>
where
    GetItem: FnMut(u32) -> Item,
{
    fn new(size: u32, get_item: GetItem) -> Self {
        Self {
            size,
            get_item,
            index: 0,
        }
    }
}

impl<Item, GetItem> Iterator for SBIterator<Item, GetItem>
where
    GetItem: FnMut(u32) -> Item,
{
    type Item = Item;
    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.size {
            self.index += 1;
            Some((self.get_item)(self.index - 1))
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.size as usize))
    }
}
