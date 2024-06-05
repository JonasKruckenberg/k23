use crate::sync::Mutex;
use core::arch::asm;
use core::ffi::CStr;
use core::fmt::{Error, Write};
use core::ops::DerefMut;
use core::{fmt, hint, slice};

const OPEN: usize = 0x01;
const WRITE: usize = 0x05;
const EXIT: usize = 0x18;
#[cfg(target_pointer_width = "32")]
const EXIT_EXTENDED: usize = 0x20;
const GET_CMDLINE: usize = 0x15;
const OPEN_W_TRUNC: usize = 4;
const OPEN_W_APPEND: usize = 8;

pub struct HostStream(usize);

impl HostStream {
    pub fn new_stdout() -> Self {
        Self::open(":tt\0", OPEN_W_TRUNC).unwrap()
    }

    pub fn new_stderr() -> Self {
        Self::open(":tt\0", OPEN_W_APPEND).unwrap()
    }

    pub fn write_all(&mut self, mut buf: &[u8]) -> Result<(), ()> {
        while !buf.is_empty() {
            match unsafe { syscall!(WRITE, self.0, buf.as_ptr(), buf.len()) } {
                // Done
                0 => return Ok(()),
                // `n` bytes were not written
                n if n <= buf.len() => {
                    let offset = (buf.len() - n) as isize;
                    buf = unsafe { slice::from_raw_parts(buf.as_ptr().offset(offset), n) }
                }
                // #[cfg(feature = "jlink-quirks")]
                // // Error (-1) - should be an error but JLink can return -1, -2, -3,...
                // // For good measure, we allow up to negative 15.
                // n if n > 0xfffffff0 => return Ok(()),
                // Error
                _ => return Err(()),
            }
        }

        Ok(())
    }

    fn open(name: &str, mode: usize) -> Result<Self, ()> {
        let name = name.as_bytes();
        match unsafe { syscall!(OPEN, name.as_ptr() as usize, mode, name.len() - 1) } as isize {
            -1 => Err(()),
            fd => Ok(Self(fd as usize)),
        }
    }
}

impl Write for HostStream {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        #[allow(clippy::default_constructed_unit_structs)]
        self.write_all(s.as_bytes()).map_err(|_| Error::default())?;

        Ok(())
    }
}

static STDOUT: Mutex<Option<HostStream>> = Mutex::new(None);
static STDERR: Mutex<Option<HostStream>> = Mutex::new(None);

pub fn with_hstdout(f: impl FnOnce(&mut HostStream)) {
    let mut stream = STDOUT.lock();

    if stream.is_none() {
        stream.replace(HostStream::new_stdout());
    }

    match stream.deref_mut() {
        Some(stream) => f(stream),
        None => unreachable!(),
    }
}

pub fn with_hstderr(f: impl FnOnce(&mut HostStream)) {
    let mut stream = STDERR.lock();

    if stream.is_none() {
        stream.replace(HostStream::new_stderr());
    }

    match stream.deref_mut() {
        Some(stream) => f(stream),
        None => unreachable!(),
    }
}

pub fn _print(args: fmt::Arguments) {
    with_hstdout(|stdout| {
        stdout
            .write_fmt(args)
            .expect("failed to write to semihosting stdout")
    })
}

pub fn _eprint(args: fmt::Arguments) {
    with_hstderr(|stderr| {
        stderr
            .write_fmt(args)
            .expect("failed to write to semihosting stderr")
    })
}

#[macro_export]
macro_rules! syscall {
    ($nr:path) => {
        syscall_inner($nr, 0)
    };
    ($nr:path, $a1:expr) => {
        syscall_inner($nr, &[$a1 as usize] as *const usize as usize)
    };
    ($nr:path, $a1:expr, $a2:expr) => {
        syscall_inner($nr, &[$a1 as usize, $a2 as usize] as *const usize as usize)
    };
    ($nr:path, $a1:expr, $a2:expr, $a3:expr) => {
        syscall_inner(
            $nr,
            &[$a1 as usize, $a2 as usize, $a3 as usize] as *const usize as usize,
        )
    };
    ($nr:path, $a1:expr, $a2:expr, $a3:expr, $a4:expr) => {
        syscall_inner(
            $nr,
            &[$a1 as usize, $a2 as usize, $a3 as usize, $a4 as usize] as *const usize as usize,
        )
    };
}

pub use syscall;

#[inline(always)]
unsafe fn syscall_inner(_nr: usize, _arg: usize) -> usize {
    cfg_if::cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            let mut nr = _nr;
            let arg = _arg;
            // The instructions below must always be uncompressed, otherwise
            // it will be treated as a regular break, hence the norvc option.
            //
            // See https://github.com/riscv/riscv-semihosting-spec for more details.
            asm!("
                .balign 16
                .option push
                .option norvc
                slli x0, x0, 0x1f
                ebreak
                srai x0, x0, 0x7
                .option pop
            ",
            inout("a0") nr,
            inout("a1") arg => _,
            options(nostack, preserves_flags),
            );
            nr
        } else {
            unimplemented!();
        }
    }
}

/// Terminates the process in an abnormal fashion.
#[cold]
pub fn abort() -> ! {
    exit(134) // SIGABRT
}

/// Terminates the current process with the specified exit code.
pub fn exit(code: i32) -> ! {
    // TODO: check sh_ext_exit_extended first
    sys_exit_extended(
        ExitReason::ADP_Stopped_ApplicationExit,
        code as isize as usize,
    );
    // If SYS_EXIT_EXTENDED is not supported, above call doesn't exit program,
    // so try again with SYS_EXIT.
    let reason = match code {
        0 => ExitReason::ADP_Stopped_ApplicationExit,
        _ => ExitReason::ADP_Stopped_RunTimeErrorUnknown,
    };
    sys_exit(reason);
    loop {
        hint::spin_loop()
    }
}

pub fn sys_exit(reason: ExitReason) {
    unsafe {
        #[cfg(target_pointer_width = "32")]
        syscall!(EXIT, reason);
        #[cfg(target_pointer_width = "64")]
        syscall!(EXIT, reason, 0);
    }
}

pub fn sys_exit_extended(reason: ExitReason, subcode: usize) {
    unsafe {
        #[cfg(target_pointer_width = "32")]
        syscall!(EXIT_EXTENDED, reason, subcode);
        #[cfg(target_pointer_width = "64")]
        syscall!(EXIT, reason, subcode);
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(usize)]
#[non_exhaustive]
#[allow(non_camel_case_types)]
pub enum ExitReason {
    // Reason codes related to hardware exceptions:
    ADP_Stopped_BranchThroughZero = 0x20000,
    ADP_Stopped_UndefinedInstr = 0x20001,
    ADP_Stopped_SoftwareInterrupt = 0x20002,
    ADP_Stopped_PrefetchAbort = 0x20003,
    ADP_Stopped_DataAbort = 0x20004,
    ADP_Stopped_AddressException = 0x20005,
    ADP_Stopped_IRQ = 0x20006,
    ADP_Stopped_FIQ = 0x20007,

    // Reason codes related to software events:
    ADP_Stopped_BreakPoint = 0x20020,
    ADP_Stopped_WatchPoint = 0x20021,
    ADP_Stopped_StepComplete = 0x20022,
    ADP_Stopped_RunTimeErrorUnknown = 0x20023,
    ADP_Stopped_InternalError = 0x20024,
    ADP_Stopped_UserInterruption = 0x20025,
    ADP_Stopped_ApplicationExit = 0x20026,
    ADP_Stopped_StackOverflow = 0x20027,
    ADP_Stopped_DivisionByZero = 0x20028,
    ADP_Stopped_OSSpecific = 0x20029,
}

pub fn get_cmdline(buf: &mut [u8]) -> Result<&CStr, core::ffi::FromBytesUntilNulError> {
    #[allow(unused)]
    struct Args<'a> {
        buf: &'a mut [u8],
        len: usize,
    }

    let mut args = Args {
        len: buf.len(),
        buf,
    };

    unsafe {
        syscall!(GET_CMDLINE, (&mut args) as *mut _ as usize);
    }

    CStr::from_bytes_until_nul(buf)
}
