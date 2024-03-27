pub mod _type;
pub mod address;
pub mod attach_info;
pub mod breakpoint;
pub mod breakpoint_location;
pub mod broadcaster;
pub mod command_interpreter;
pub mod command_return_object;
pub mod compile_unit;
pub mod data;
pub mod debugger;
pub mod error;
pub mod event;
pub mod execution_context;
pub mod file;
pub mod file_spec;
pub mod frame;
pub mod instruction;
pub mod instruction_list;
pub mod launch_info;
pub mod line_entry;
pub mod listener;
pub mod memory_region_info;
pub mod memory_region_info_list;
pub mod module;
pub mod module_spec;
pub mod platform;
pub mod process;
pub mod section;
pub mod stream;
pub mod string_list;
pub mod structured_data;
pub mod symbol;
pub mod symbol_context;
pub mod symbol_context_list;
pub mod target;
pub mod thread;
pub mod value;
pub mod value_list;
pub mod watchpoint;

use cpp::cpp;

cpp! {{
    #ifdef _WIN32
        #define _CRT_NONSTDC_NO_DEPRECATE 1
        #include <io.h>
        #include <fcntl.h>
    #endif
    #include <stdio.h>
    #include <lldb/API/LLDB.h>
    using namespace lldb;
}}
