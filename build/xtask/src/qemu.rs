use crate::profile::{Architecture, Profile};
use clap::Parser;
use std::path::Path;
use std::process::{Child, Command, Stdio};

#[derive(Debug, Parser)]
pub struct QemuOptions {
    /// Listen for GDB connections and wait for a debugger to attach.
    #[clap(long, short)]
    pub wait_for_debugger: bool,
    /// The TCP port to listen for debug connections on.
    #[clap(long, default_value = "1234")]
    pub gdb_port: u16,
    /// Extra arguments passed to QEMU.
    #[clap(raw = true)]
    pub qemu_args: Vec<String>,
}

pub fn run(
    qemu: &QemuOptions,
    profile: Profile,
    image: &Path,
    inherit_stdio: bool,
    separate_thread: bool,
) -> crate::Result<()> {
    let mut cmd = match profile.arch {
        Architecture::Riscv64 => {
            let mut cmd = Command::new("qemu-system-riscv64");
            cmd.args([
                "-machine",
                "virt",
                "-cpu",
                "rv64",
                "-m",
                "256M",
                "-d",
                "guest_errors",
                "-display",
                "none",
                "-serial",
                "mon:stdio",
                "-semihosting-config",
                "enable=on,target=native",
                "-smp",
                "cpus=1",
                // "-smp",
                // "cpus=8",
                // "-object",
                // "memory-backend-ram,size=128M,id=m0",
                // "-object",
                // "memory-backend-ram,size=128M,id=m1",
                // "-numa",
                // "node,cpus=0-3,nodeid=0,memdev=m0",
                // "-numa",
                // "node,cpus=4-7,nodeid=1,memdev=m1",
                // "-numa",
                // "dist,src=0,dst=1,val=20",
                // "-monitor",
                // "unix:qemu-monitor-socket,server,nowait",
                "-kernel",
                image.to_str().unwrap(),
            ]);
            cmd.args(&qemu.qemu_args);
            cmd
        }
    };

    if qemu.wait_for_debugger {
        cmd.arg("-S")
            .arg("-gdb")
            .arg(format!("tcp::{}", qemu.gdb_port));
    }

    if inherit_stdio {
        cmd.stderr(Stdio::inherit());
        cmd.stdout(Stdio::inherit());
        cmd.stdin(Stdio::inherit());
    } else {
        cmd.stderr(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stdin(Stdio::null());
    }

    let mut run = move || {
        let mut qemu = KillOnDrop(cmd.spawn().expect("Failed to spawn qemu. Is it installed?"));
        qemu.0.wait().unwrap();
    };

    if separate_thread {
        std::thread::spawn(run);
    } else {
        run();
    }

    Ok(())
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        self.0.kill().ok();
    }
}
