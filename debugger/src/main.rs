use lldb::{
    ProcessState, SBAttachInfo, SBCommandReturnObject, SBDebugger, SBExecutionContext, SBListener,
    SBPlatform, SBPlatformConnectOptions, SBTarget, SBThread, StopReason,
};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    SBDebugger::initialize();

    let dbg = SBDebugger::create(false);
    dbg.set_async_mode(false);

    let cmd = dbg.command_interpreter();
    let mut ctx = SBExecutionContext::new();

    cmd.handle_command_with_context("command script import '/Users/jonas/.rustup/toolchains/nightly-aarch64-apple-darwin/lib/rustlib/etc/lldb_lookup.py'", &mut ctx,false)?;
    cmd.handle_command_with_context("command source -s 0 '/Users/jonas/.rustup/toolchains/nightly-aarch64-apple-darwin/lib/rustlib/etc/lldb_commands'", &mut ctx,false)?;

    // let out = cmd.handle_command_with_context(
    //     "target create 'target/riscv64gc-unknown-none-elf/debug/loader'",
    //     &mut ctx,
    //     false,
    // )?;
    // println!("1: {}", out.to_str().unwrap());
    // let out = cmd.handle_command_with_context("gdb-remote localhost:1234", &mut ctx, false)?;
    // println!("2: {}", out.to_str().unwrap());

    let target = dbg.create_target(
        Path::new("target/riscv64gc-unknown-none-elf/debug/loader"),
        None,
        None,
        false,
    )?;

    let process = target.connect_remote("connect://localhost:1234", "gdb-remote")?;

    let thread = process.selected_thread();
    let frame = thread.selected_frame();

    println!("Process {} {:?}", process.process_id(), process.state());
    println!(
        "* thread #{}, stop reason = {}",
        thread.thread_id(),
        thread.stop_description()
    );
    println!("frame #{}: {:#016x}", frame.frame_id(), frame.pc());

    println!("{}", frame.disassemble());

    SBDebugger::terminate();

    Ok(())
}
