use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut};
use core::{ops, slice};
use gimli::{
    BaseAddresses, CfaRule, EhFrame, EndianSlice, NativeEndian, Register, RegisterRule, RiscV,
    UnwindContext, UnwindSection, UnwindTableRow,
};

pub struct Backtrace {
    bases: BaseAddresses,
    eh_frame: EhFrame<EndianSlice<'static, NativeEndian>>,
    ctx: Context,
    unwinder: UnwindContext<EndianSlice<'static, NativeEndian>, StoreOnStack>,
}

impl Backtrace {
    pub fn from_context(ctx: Context) -> Self {
        extern "C" {
            static __eh_frame_start: u8;
            static __eh_frame_end: u8;
        }

        let eh_frame = unsafe {
            let start = addr_of!(__eh_frame_start);
            let end = addr_of!(__eh_frame_end);

            slice::from_raw_parts(start, end as usize - start as usize)
        };

        let bases = BaseAddresses::default()
            .set_eh_frame(eh_frame.as_ptr() as _)
            .set_text(0x80200000);

        let eh_frame = EhFrame::new(eh_frame, NativeEndian);

        let unwinder = UnwindContext::<_, StoreOnStack>::new_in();

        Self {
            bases,
            eh_frame,
            ctx,
            unwinder,
        }
    }

    fn construct_frame(&mut self, pc: usize) -> Option<Frame> {
        let ra = self.ctx[RiscV::RA];

        if ra == 0 {
            return None;
        }

        let fde = self
            .eh_frame
            .fde_for_address(&self.bases, pc as _, EhFrame::cie_from_offset)
            .unwrap();

        let row = fde
            .unwind_info_for_address(&self.eh_frame, &self.bases, &mut self.unwinder, ra as _)
            .unwrap();

        Some(Frame {
            row: row.clone(),
            pc: ra,
        })
    }

    fn update_regs_from_frame(&mut self, frame: &Frame) {
        let row = &frame.row;

        let cfa = match *row.cfa() {
            CfaRule::RegisterAndOffset { register, offset } => {
                self.ctx[register].wrapping_add(offset as usize)
            }
            CfaRule::Expression(_) => panic!("DWARF expressions are unsupported"),
        };

        self.ctx[RiscV::SP] = cfa as _;
        self.ctx[RiscV::RA] = 0;

        for (reg, rule) in row.registers() {
            let value = match *rule {
                RegisterRule::Undefined | RegisterRule::SameValue => self.ctx[*reg],
                RegisterRule::Offset(offset) => unsafe {
                    *((cfa.wrapping_add(offset as usize)) as *const usize)
                },
                RegisterRule::ValOffset(offset) => cfa.wrapping_add(offset as usize),
                RegisterRule::Register(r) => self.ctx[r],
                RegisterRule::Expression(_) | RegisterRule::ValExpression(_) => {
                    panic!("DWARF expressions are unsupported")
                }
                RegisterRule::Architectural => unreachable!(),
                RegisterRule::Constant(value) => value as usize,
                _ => unreachable!(),
            };
            self.ctx[*reg] = value;
        }
    }
}

impl Iterator for Backtrace {
    type Item = Frame;

    fn next(&mut self) -> Option<Self::Item> {
        let frame = self.construct_frame(self.ctx.ra)?;
        self.update_regs_from_frame(&frame);

        Some(frame)
    }
}

#[repr(C)]
#[derive(Clone, Default)]
pub struct Context {
    pub ra: usize,
    pub sp: usize,
}

impl Context {
    pub fn capture() -> Self {
        let (ra, sp);
        unsafe {
            asm!(
                "mv {}, ra",
                "mv {}, sp",
                out(reg) ra,
                out(reg) sp,
            );
        }
        Self { ra, sp }
    }
}

pub struct Frame {
    pub pc: usize,
    row: UnwindTableRow<EndianSlice<'static, NativeEndian>, StoreOnStack>,
}

impl ops::Index<Register> for Context {
    type Output = usize;

    fn index(&self, index: Register) -> &Self::Output {
        match index {
            Register(1) => &self.ra,
            Register(2) => &self.sp,
            _ => panic!("unsupported register"),
        }
    }
}

impl ops::IndexMut<Register> for Context {
    fn index_mut(&mut self, index: Register) -> &mut Self::Output {
        match index {
            Register(1) => &mut self.ra,
            Register(2) => &mut self.sp,
            _ => panic!("unsupported register"),
        }
    }
}

pub fn with_context<T, F: FnOnce(&mut Context) -> T>(f: F) -> T {
    use core::mem::ManuallyDrop;

    union Data<T, F> {
        f: ManuallyDrop<F>,
        t: ManuallyDrop<T>,
    }

    extern "C" fn delegate<T, F: FnOnce(&mut Context) -> T>(ctx: &mut Context, ptr: *mut ()) {
        // SAFETY: This function is called exactly once; it extracts the function, call it and
        // store the return value. This function is `extern "C"` so we don't need to worry about
        // unwinding past it.
        unsafe {
            let data = &mut *ptr.cast::<Data<T, F>>();
            let t = ManuallyDrop::take(&mut data.f)(ctx);
            data.t = ManuallyDrop::new(t);
        }
    }

    let mut data = Data {
        f: ManuallyDrop::new(f),
    };
    save_context(delegate::<T, F>, addr_of_mut!(data).cast());
    unsafe { ManuallyDrop::into_inner(data.t) }
}

#[naked]
extern "C-unwind" fn save_context(f: extern "C" fn(&mut Context, *mut ()), ptr: *mut ()) {
    unsafe {
        asm!(
            "
            mv t0, sp
            add sp, sp, -({sizeof_context} + 16)
            sd ra, {sizeof_context}(sp)
            ",
            "
            sd ra, 0(sp)
            sd t0, 8(sp)
            ",
            "
            mv t0, a0
            mv a0, sp
            jalr t0
            ld ra, {sizeof_context}(sp)
            add sp, sp, ({sizeof_context} + 16)
            ret
            ",
            sizeof_context = const core::mem::size_of::<Context>(),
            options(noreturn)
        );
    }
}

struct StoreOnStack;

// gimli's MSRV doesn't allow const generics, so we need to pick a supported array size.
const fn next_value(x: usize) -> usize {
    let supported = [0, 1, 2, 3, 4, 8, 16, 32, 64, 128];
    let mut i = 0;
    while i < supported.len() {
        if supported[i] >= x {
            return supported[i];
        }
        i += 1;
    }
    192
}

const MAX_REG_RULES: usize = 65;

impl<R: gimli::Reader> gimli::UnwindContextStorage<R> for StoreOnStack {
    type Rules = [(Register, RegisterRule<R>); next_value(MAX_REG_RULES)];
    type Stack = [UnwindTableRow<R, Self>; 2];
}
