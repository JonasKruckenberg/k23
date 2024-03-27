use crate::strings::{get_str, with_cstr};
use crate::{
    IsValid, ProcessID, SBAttachInfo, SBDebugger, SBError, SBLaunchInfo, SBProcess, SBTarget,
};
use cpp::{cpp, cpp_class};
use std::ffi;
use std::path::Path;

/// The SBPlatform class represents the current host, or a remote host.
/// It can be connected to a remote platform in order to provide ways to remotely launch and attach to processes,
/// upload/download files, create directories, run remote shell commands, find locally cached versions
/// of files from the remote system, and much more.
///
/// SBPlatform objects can be created and then used to connect to a remote platform which allows the
/// SBPlatform to be used to get a list of the current processes on the remote host, attach to one of those processes,
/// install programs on the remote system, attach and launch processes, and much more.
///
/// Every SBTarget has a corresponding SBPlatform. The platform can be specified upon target creation,
/// or the currently selected platform will attempt to be used when creating the target automatically
/// as long as the currently selected platform matches the target architecture and executable type.
/// If the architecture or executable type do not match, a suitable platform will be found automatically.
cpp_class!(pub unsafe struct SBPlatform as "SBPlatform");

unsafe impl Send for SBPlatform {}

impl SBPlatform {
    pub fn new(platform_name: &str) -> Self {
        with_cstr(platform_name, |platform_name| {
            cpp!(unsafe [platform_name as "const char*"] -> SBPlatform as "SBPlatform" {
                return SBPlatform(platform_name);
            })
        })
    }

    pub fn clear(&self) {
        cpp!(unsafe [self as "SBPlatform*"] {
            return self->Clear();
        })
    }

    pub fn name(&self) -> &str {
        let ptr = cpp!(unsafe [self as "SBPlatform*"] -> *const ffi::c_char as "const char*" {
            return self->GetName();
        });
        unsafe { get_str(ptr) }
    }

    pub fn connect_remote(
        &self,
        connect_options: &SBPlatformConnectOptions,
    ) -> Result<(), SBError> {
        cpp!(unsafe  [self as "SBPlatform*", connect_options as "SBPlatformConnectOptions*"] -> SBError as "SBError" {
            return self->ConnectRemote(*connect_options);
        }).into_result()
    }

    pub fn disconnect_remote(&self) {
        cpp!(unsafe [self as "SBPlatform*"] {
            self->DisconnectRemote();
        })
    }

    pub fn is_connected(&self) -> bool {
        cpp!(unsafe [self as "SBPlatform*"] -> bool as "bool" {
            return self->IsConnected();
        })
    }

    pub fn triple(&self) -> &str {
        let ptr = cpp!(unsafe [self as "SBPlatform*"] -> *const ffi::c_char as "const char*" {
            return self->GetTriple();
        });
        unsafe { get_str(ptr) }
    }

    pub fn hostname(&self) -> &str {
        let ptr = cpp!(unsafe [self as "SBPlatform*"] -> *const ffi::c_char as "const char*" {
            return self->GetHostname();
        });
        unsafe { get_str(ptr) }
    }

    pub fn os_build(&self) -> &str {
        let ptr = cpp!(unsafe [self as "SBPlatform*"] -> *const ffi::c_char as "const char*" {
            return self->GetOSBuild();
        });
        unsafe { get_str(ptr) }
    }

    pub fn os_description(&self) -> &str {
        let ptr = cpp!(unsafe [self as "SBPlatform*"] -> *const ffi::c_char as "const char*" {
            return self->GetOSDescription();
        });
        unsafe { get_str(ptr) }
    }

    pub fn get_file_permissions(&self, path: &Path) -> u32 {
        with_cstr(path.to_str().unwrap(), |path| {
            cpp!(unsafe [self as "SBPlatform*", path as "const char*"] -> u32 as "uint32_t" {
                return self->GetFilePermissions(path);
            })
        })
    }

    pub fn launch(&self, launch_info: &SBLaunchInfo) -> Result<(), SBError> {
        cpp!(unsafe [self as "SBPlatform*", launch_info as "SBLaunchInfo*"] -> SBError as "SBError" {
            return self->Launch(*launch_info);
        })
            .into_result()
    }

    pub fn kill(&self, pid: ProcessID) -> Result<(), SBError> {
        cpp!(unsafe [self as "SBPlatform*", pid as "lldb::pid_t"] -> SBError as "SBError" {
            return self->Kill(pid);
        })
        .into_result()
    }

    // pub fn attach(
    //     &self,
    //     attach_info: &SBAttachInfo,
    //     debugger: &SBDebugger,
    //     target: &SBTarget,
    // ) -> Result<SBProcess, SBError> {
    //     let mut error = SBError::new();
    //
    //     let process = cpp!(unsafe [self as "SBPlatform*", attach_info as "SBAttachInfo*", debugger as "SBDebugger*", target as "SBTarget*", mut error as "SBError"] -> SBProcess as "SBProcess" {
    //         return self->Attach(*attach_info, debugger, target, error);
    //     });
    //
    //     if error.is_success() {
    //         if process.is_valid() {
    //             Ok(process)
    //         } else {
    //             error.set_error_string("Attach failed.");
    //             Err(error)
    //         }
    //     } else {
    //         Err(error)
    //     }
    // }
}

impl IsValid for SBPlatform {
    fn is_valid(&self) -> bool {
        cpp!(unsafe [self as "SBPlatform*"] -> bool as "bool" {
            return self->IsValid();
        })
    }
}

cpp_class!(pub unsafe struct SBPlatformConnectOptions as "SBPlatformConnectOptions");

unsafe impl Send for SBPlatformConnectOptions {}

impl SBPlatformConnectOptions {
    pub fn new(url: &str) -> Self {
        with_cstr(url, |url| {
            cpp!(unsafe [url as "const char*"] -> SBPlatformConnectOptions as "SBPlatformConnectOptions" {
                return SBPlatformConnectOptions(url);
            })
        })
    }
    pub fn url(&self) -> &str {
        let ptr = cpp!(unsafe [self as "SBPlatformConnectOptions*"] -> *const ffi::c_char as "const char*" {
            return self->GetURL();
        });
        unsafe { get_str(ptr) }
    }
    pub fn set_url(&self, url: &str) {
        with_cstr(url, |url| {
            cpp!(unsafe [self as "SBPlatformConnectOptions*", url as "const char*"] {
                self->SetURL(url);
            });
        })
    }
    pub fn rsync_enabled(&self) -> bool {
        cpp!(unsafe [self as "SBPlatformConnectOptions*"] -> bool as "bool" {
            return self->GetRsyncEnabled();
        })
    }
    pub fn enable_rsync(
        &self,
        options: &str,
        remote_path_prefix: &str,
        omit_remote_hostname: bool,
    ) {
        with_cstr(options, |options| {
            with_cstr(remote_path_prefix, |remote_path_prefix| {
                cpp!(unsafe [self as "SBPlatformConnectOptions*", options as "const char *",
                            remote_path_prefix as "const char *", omit_remote_hostname as "bool"] {
                    self->EnableRsync(options, remote_path_prefix, omit_remote_hostname);
                });
            })
        })
    }
    pub fn disable_rsync(&self) {
        cpp!(unsafe [self as "SBPlatformConnectOptions*"]  {
            return self->DisableRsync();
        })
    }
    pub fn local_cache_directory(&self) -> &str {
        let ptr = cpp!(unsafe [self as "SBPlatformConnectOptions*"] -> *const ffi::c_char as "const char*" {
            return self->GetLocalCacheDirectory();
        });
        unsafe { get_str(ptr) }
    }
    pub fn set_local_cache_directory(&self, path: &str) {
        with_cstr(path, |path| {
            cpp!(unsafe [self as "SBPlatformConnectOptions*", path as "const char*"] {
                self->SetLocalCacheDirectory(path);
            });
        })
    }
}

cpp_class!(pub unsafe struct SBPlatformShellCommand as "SBPlatformShellCommand");

unsafe impl Send for SBPlatformShellCommand {}

impl SBPlatformShellCommand {
    pub fn new(command: &str) -> Self {
        with_cstr(command, |command| {
            cpp!(unsafe [command as "const char*"] -> SBPlatformShellCommand as "SBPlatformShellCommand" {
                return SBPlatformShellCommand(command);
            })
        })
    }
    pub fn clear(&self) {
        cpp!(unsafe [self as "SBPlatformShellCommand*"] {
            return self->Clear();
        })
    }
    pub fn command(&self) -> &str {
        let ptr = cpp!(unsafe [self as "SBPlatformShellCommand*"] -> *const ffi::c_char as "const char*" {
            return self->GetCommand();
        });
        unsafe { get_str(ptr) }
    }
    pub fn set_command(&self, command: &str) {
        with_cstr(command, |command| {
            cpp!(unsafe [self as "SBPlatformShellCommand*", command as "const char*"] {
                self->SetCommand(command);
            });
        })
    }
    pub fn working_directory(&self) -> &str {
        let ptr = cpp!(unsafe [self as "SBPlatformShellCommand*"] -> *const ffi::c_char as "const char*" {
            return self->GetWorkingDirectory();
        });
        unsafe { get_str(ptr) }
    }
    pub fn set_working_directory(&self, path: &str) {
        with_cstr(path, |path| {
            cpp!(unsafe [self as "SBPlatformShellCommand*", path as "const char*"] {
                self->SetWorkingDirectory(path);
            });
        })
    }
    pub fn timeout_seconds(&self) -> u32 {
        cpp!(unsafe [self as "SBPlatformShellCommand*"] -> u32 as "uint32_t" {
            return self->GetTimeoutSeconds();
        })
    }
    pub fn set_timeout_seconds(&self, sec: u32) {
        cpp!(unsafe [self as "SBPlatformShellCommand*", sec as "uint32_t"] {
            self->SetTimeoutSeconds(sec);
        });
    }
    pub fn signal(&self) -> i32 {
        cpp!(unsafe [self as "SBPlatformShellCommand*"] -> ffi::c_int as "int" {
            return self->GetSignal();
        }) as i32
    }
    pub fn status(&self) -> i32 {
        cpp!(unsafe [self as "SBPlatformShellCommand*"] -> ffi::c_int as "int" {
            return self->GetStatus();
        }) as i32
    }
    pub fn output(&self) -> &str {
        let ptr = cpp!(unsafe [self as "SBPlatformShellCommand*"] -> *const ffi::c_char as "const char*" {
            return self->GetOutput();
        });
        unsafe { get_str(ptr) }
    }
}
