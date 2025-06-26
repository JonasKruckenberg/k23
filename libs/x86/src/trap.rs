// Copyright 2025 bubblepipe
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! x86_64 exception and interrupt types

/// x86_64 exception types
/// source: http://wiki.osdev.org/Exceptions
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Exception {
    DivideError = 0,
    Debug = 1,
    NonmaskableExternInterrupt = 2,
    Breakpoint = 3,
    Overflow = 4,
    BoundRangeExceeded = 5,
    InvalidOpcode = 6,
    DeviceNotAvailable = 7,
    DoubleFault = 8,
    CoprocessorSegmentOverrun = 9,
    InvalidTSS = 10,
    SegmentNotPresent = 11,
    StackSegmentFault = 12,
    GeneralProtectionFault = 13,
    PageFault = 14,
    Reserved15 = 15,
    X87FloatingPointException = 16,
    AlignmentCheck = 17,
    MachineCheck = 18,
    SIMDFloatingPointException = 19,
    VirtualizationException = 20,
    ControlProtectionException = 21,
    Reserved22 = 22,
    Reserved23 = 23,
    Reserved24 = 24,
    Reserved25 = 25,
    Reserved26 = 26,
    Reserved27 = 27,
    HypervisorInjectionException = 28,
    VMMCommunicationException = 29,
    SecurityException = 30,
    Reserved31 = 31,
}

/// x86_64 interrupt types
/// source: http://wiki.osdev.org/Interrupts
/// source: http://wiki.osdev.org/8259_PIC
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Interrupt {
    Timer = 0x20,
    Keyboard = 0x21,
    Cascade = 0x22,
    Serial2 = 0x23,
    Serial1 = 0x24,
    Parallel2 = 0x25,
    Floppy = 0x26,
    Parallel1 = 0x27,
    RealTimeClock = 0x28,
    Peripheral29 = 0x29,
    Peripheral2A = 0x2A,
    Peripheral2B = 0x2B,
    Mouse = 0x2C,
    Coprocessor = 0x2D,
    ATA1 = 0x2E,
    ATA2 = 0x2F,
}

/// x86_64 trap type using the generic trap library
pub type Trap = trap::Trap<Interrupt, Exception>;