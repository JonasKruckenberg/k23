// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::{format, vec};
use core::fmt::{Display, LowerHex};

use anyhow::{Context, anyhow, bail};
use spin::Mutex;
use wasmparser::Validator;
use wast::core::{EncodeOptions, NanPattern, V128Pattern, WastArgCore, WastRetCore};
use wast::parser::ParseBuffer;
use wast::token::{F32, F64};
use wast::{
    Error, QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastInvoke, WastRet, Wat, parser,
};

use crate::wasm::{
    ConstExprEvaluator, Engine, Extern, Instance, Linker, Module, PlaceholderAllocatorDontUse,
    Store, Val, host_funcs,
};

macro_rules! wast_tests {
    ($($names:ident $paths:literal,)*) => {
        $(
            #[ktest::test]
            async fn $names() {
                let mut ctx = $crate::tests::wast::WastContext::new_default().unwrap();

                ctx.run($paths, include_str!($paths)).await.unwrap();
            }
        )*
    };
}
pub(crate) use wast_tests;

enum Outcome<T = Vec<Val>> {
    Ok(T),
    Trap(anyhow::Error),
}

impl<T> Outcome<T> {
    fn map<U>(self, map: impl FnOnce(T) -> U) -> Outcome<U> {
        match self {
            Outcome::Ok(t) => Outcome::Ok(map(t)),
            Outcome::Trap(t) => Outcome::Trap(t),
        }
    }

    fn into_result(self) -> anyhow::Result<T> {
        match self {
            Outcome::Ok(t) => Ok(t),
            Outcome::Trap(t) => Err(t),
        }
    }
}

pub struct WastContext(Arc<Mutex<WastContextInner>>);
pub struct WastContextInner {
    engine: Engine,
    store: Store<()>,
    linker: Linker<()>,
    const_eval: ConstExprEvaluator,
    validator: Validator,
    current: Option<Instance>,
}

impl WastContext {
    pub fn new_default() -> crate::Result<Self> {
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        let store = Store::new(&engine, &PlaceholderAllocatorDontUse, ());

        linker.func_wrap("spectest", "print", || {})?;
        linker.func_wrap("spectest", "print_i32", move |val: i32| {
            tracing::debug!("{val}: i32")
        })?;
        linker.func_wrap("spectest", "print_i64", move |val: i64| {
            tracing::debug!("{val}: i64")
        })?;
        linker.func_wrap("spectest", "print_f32", move |val: f32| {
            tracing::debug!("{val}: f32")
        })?;
        linker.func_wrap("spectest", "print_f64", move |val: f64| {
            tracing::debug!("{val}: f64")
        })?;
        linker.func_wrap("spectest", "print_i32_f32", move |i: i32, f: f32| {
            tracing::debug!("{i}: i32");
            tracing::debug!("{f}: f32");
        })?;
        linker.func_wrap("spectest", "print_f64_f64", move |f1: f64, f2: f64| {
            tracing::debug!("{f1}: f64");
            tracing::debug!("{f2}: f64");
        })?;

        // used in the hostfunc smoke tests
        linker.func_wrap("k23", "roundtrip_i64", |arg: u64| -> u64 {
            tracing::debug!("Hello World from hostfunc!");
            arg
        })?;
        
        // Register WASI host functions for testing
        host_funcs::register_host_functions(&mut linker)?;

        // let ty = GlobalType {
        //     content_type: ValType::I32,
        //     mutable: false,
        //     shared: false,
        // };
        // ctx.linker.define(
        //     ctx.store,
        //     "spectest",
        //     "global_i32",
        //     Global::new(ty, Value::I32(666)),
        // )?;

        // let ty = GlobalType {
        //     content_type: ValType::I64,
        //     mutable: false,
        //     shared: false,
        // };
        // ctx.linker.define(
        //     ctx.store,
        //     "spectest",
        //     "global_i64",
        //     Global::new(ty, Value::I64(666)),
        // )?;

        // let ty = GlobalType {
        //     content_type: ValType::F32,
        //     mutable: false,
        //     shared: false,
        // };
        // ctx.linker.define(
        //     ctx.store,
        //     "spectest",
        //     "global_f32",
        //     Global::new(ty, Value::F32(f32::from_bits(0x4426_a666u32))),
        // )?;

        // let ty = GlobalType {
        //     content_type: ValType::F64,
        //     mutable: false,
        //     shared: false,
        // };
        // ctx.linker.define(
        //     ctx.store,
        //     "spectest",
        //     "global_f64",
        //     Global::new(ty, Value::F64(f64::from_bits(0x4084_d4cc_cccc_cccd))),
        // )?;

        // let ty = TableType {
        //     element_type: RefType::FUNCREF,
        //     table64: false,
        //     initial: 10,
        //     maximum: Some(20),
        //     shared: false,
        // };
        // ctx.linker.define(
        //     ctx.store,
        //     "spectest",
        //     "table",
        //     Table::new(ty, Ref::Func(None)),
        // )?;

        // let ty = MemoryType {
        //     memory64: false,
        //     shared: false,
        //     initial: 1,
        //     maximum: Some(2),
        //     page_size_log2: None,
        // };
        // ctx.linker
        //     .define(&mut ctx.store, "spectest", "memory", Memory::new(ty))?;

        Ok(Self(Arc::new(Mutex::new(WastContextInner {
            engine,
            linker,
            store,
            const_eval: ConstExprEvaluator::default(),
            validator: Validator::new(),
            current: None,
        }))))
    }

    pub async fn run(&mut self, path: &str, wat: &str) -> crate::Result<()> {
        let buf = ParseBuffer::new(&wat)?;
        let wast = parser::parse::<Wast>(&buf)?;
        for directive in wast.directives {
            let span = directive.span();
            let (line, col) = span.linecol_in(wat);
            self.run_directive(directive, path, &wat)
                .await
                .with_context(|| format!("location ({path}:{line}:{col})"))?;
        }
        Ok(())
    }

    async fn run_directive(
        &mut self,
        directive: WastDirective<'_>,
        path: &str,
        wat: &str,
    ) -> crate::Result<()> {
        tracing::trace!("{directive:?}");
        match directive {
            WastDirective::Module(module) => self.module(module, path, wat)?,
            WastDirective::Register { name, module, .. } => {
                self.register(module.map(|s| s.name()), name)?;
            }
            WastDirective::Invoke(i) => {
                self.perform_invoke(i).await?;
            }
            WastDirective::AssertMalformed { module, .. } => {
                if let Ok(()) = self.module(module, path, wat) {
                    bail!("expected malformed module to fail to instantiate");
                }
            }
            WastDirective::AssertInvalid {
                module, message, ..
            } => {
                let err = match self.module(module, path, wat) {
                    Ok(()) => {
                        tracing::error!("expected module to fail to build");
                        return Ok(());
                    }
                    Err(e) => e,
                };
                let error_message = format!("{err:?}");

                if !error_message.contains(message) {
                    bail!(
                        "assert_invalid: expected {}, got {}",
                        message,
                        error_message
                    )
                }
            }
            WastDirective::AssertUnlinkable {
                module, message, ..
            } => {
                let err = match self.module(QuoteWat::Wat(module), path, wat) {
                    Ok(()) => bail!("expected module to fail to link"),
                    Err(e) => e,
                };
                let error_message = format!("{err:?}");
                if !error_message.contains(message) {
                    bail!(
                        "assert_unlinkable: expected {}, got {}",
                        message,
                        error_message
                    )
                }
            }
            WastDirective::AssertTrap { exec, message, .. } => {
                let result = self.perform_execute(exec).await?;
                self.assert_trap(result, message)?;
            }
            WastDirective::AssertReturn { exec, results, .. } => {
                let result = self.perform_execute(exec).await?;
                self.assert_return(result, &results)?;
            }
            WastDirective::AssertExhaustion { call, message, .. } => {
                let result = self.perform_invoke(call).await?;
                self.assert_trap(result, message)?;
            }
            WastDirective::ModuleDefinition(_) => {}
            WastDirective::ModuleInstance { .. } => {}
            WastDirective::AssertException { .. } => {}
            WastDirective::AssertSuspension { .. } => {}
            WastDirective::Thread(_) => {}
            WastDirective::Wait { .. } => {}
        }

        Ok(())
    }

    fn inner_mut(&mut self) -> &mut WastContextInner {
        Mutex::get_mut(Arc::get_mut(&mut self.0).unwrap())
    }

    fn module(&mut self, mut wat: QuoteWat, _path: &str, _raw: &str) -> anyhow::Result<()> {
        let encode_wat = |wat: &mut Wat<'_>| -> anyhow::Result<Vec<u8>> {
            Ok(EncodeOptions::default()
                // TODO .dwarf(path, raw, GenerateDwarf::Full)
                .encode_wat(wat)?)
        };

        let bytes = match &mut wat {
            QuoteWat::Wat(wat) => encode_wat(wat)?,
            QuoteWat::QuoteModule(_, source) => {
                let mut text = Vec::new();
                for (_, src) in source {
                    text.extend_from_slice(src);
                    text.push(b' ');
                }
                let text = core::str::from_utf8(&text).map_err(|_| {
                    let span = wat.span();
                    Error::new(span, "malformed UTF-8 encoding".to_string())
                })?;
                let buf = ParseBuffer::new(text)?;
                let mut wat = parser::parse::<Wat<'_>>(&buf)?;
                encode_wat(&mut wat)?
            }
            QuoteWat::QuoteComponent(_, _) => unimplemented!(),
        };

        let instance = match self.instantiate_module(&bytes)? {
            Outcome::Ok(i) => i,
            Outcome::Trap(e) => return Err(e).context("instantiation failed"),
        };

        let inner = self.inner_mut();
        if let Some(name) = wat.name() {
            inner
                .linker
                .define_instance(&mut inner.store, name.name(), instance)?;
        }
        inner.current.replace(instance);

        Ok(())
    }

    fn register(&mut self, name: Option<&str>, as_name: &str) -> anyhow::Result<()> {
        let inner = self.inner_mut();
        if let Some(name) = name {
            inner.linker.alias_module(name, as_name)?
        } else {
            let current = inner.current.as_ref().context("no previous instance")?;
            inner
                .linker
                .define_instance(&mut inner.store, as_name, *current)?
        };

        Ok(())
    }

    async fn perform_invoke(&mut self, exec: WastInvoke<'_>) -> anyhow::Result<Outcome> {
        let export = self.get_export(exec.module.map(|i| i.name()), exec.name)?;
        let func = export
            .into_func()
            .ok_or_else(|| anyhow!("no function named `{}`", exec.name))?;

        let values = exec
            .args
            .iter()
            .map(|v| match v {
                WastArg::Core(v) => wast_arg_to_val(v),
                // WastArg::Component(_) => bail!("expected component function, found core"),
                _ => unreachable!(),
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let inner = self.inner_mut();
        let ty = func.ty(&mut inner.store);
        let this = self.0.clone();

        // FIXME the virtual memory subsystem trap handling code will look for a current task
        //  in order to find the current address space to resole page faults against. This is why
        //  we need to wrap this call in a `spawn` that we immediately await (so the scheduling
        //  subsystem tracks it as a task). Ideally we would get rid of this and have some other
        //  mechanism of tracking the current address space...
        // scheduler()
        //     .spawn(async move {
        let mut results = vec![Val::I32(0); ty.results().len()];

        match func.call(&mut this.lock().store, &values, &mut results) {
            Ok(()) => Ok(Outcome::Ok(results)),
            Err(e) => Ok(Outcome::Trap(e.into())),
        }
        // })
        // .await
        // .unwrap()
    }

    async fn perform_execute(&mut self, exec: WastExecute<'_>) -> anyhow::Result<Outcome> {
        match exec {
            WastExecute::Invoke(invoke) => self.perform_invoke(invoke).await,
            WastExecute::Wat(mut module) => Ok(match &mut module {
                Wat::Module(m) => self.instantiate_module(&m.encode()?)?.map(|_| Vec::new()),
                _ => unimplemented!(),
            }),
            WastExecute::Get { module, global, .. } => {
                self.get_global(module.map(|s| s.name()), global)
            }
        }
    }

    fn assert_return(&mut self, result: Outcome, results: &[WastRet<'_>]) -> anyhow::Result<()> {
        let values = result.into_result()?;
        if values.len() != results.len() {
            bail!("expected {} results found {}", results.len(), values.len());
        }
        for (v, e) in values.iter().zip(results) {
            let e = match e {
                WastRet::Core(core) => core,
                // WastRet::Component(_) => {
                //     bail!("expected component value found core value")
                // }
                _ => unreachable!(),
            };

            let inner = self.inner_mut();
            match_val(&mut inner.store, v, e)?;
        }

        Ok(())
    }

    fn assert_trap(&self, result: Outcome, expected: &str) -> anyhow::Result<()> {
        let trap = match result {
            Outcome::Ok(values) => bail!("expected trap, got {:?}", values),
            Outcome::Trap(t) => t,
        };
        let actual = format!("{trap:?}");
        if actual.contains(expected)
            // `bulk-memory-operations/bulk.wast` checks for a message that
            // specifies which element is uninitialized, but our trap_handling don't
            // shepherd that information out.
            || (expected.contains("uninitialized element 2") && actual.contains("uninitialized element"))
            // function references call_ref
            || (expected.contains("null function") && (actual.contains("uninitialized element") || actual.contains("null reference")))
        {
            return Ok(());
        }
        bail!("expected '{}', got '{}'", expected, actual)
    }

    fn instantiate_module(&mut self, module: &[u8]) -> anyhow::Result<Outcome<Instance>> {
        let inner = self.inner_mut();
        let module = Module::from_bytes(&inner.engine, &mut inner.validator, module)?;

        Ok(
            match inner
                .linker
                .instantiate(&mut inner.store, &mut inner.const_eval, &module)
            {
                Ok(i) => Outcome::Ok(i),
                Err(e) => Outcome::Trap(e.into()),
            },
        )
    }

    /// Get the value of an exported global from an instance.
    fn get_global(&mut self, instance_name: Option<&str>, field: &str) -> anyhow::Result<Outcome> {
        let ext = self.get_export(instance_name, field)?;
        let global = ext
            .into_global()
            .ok_or_else(|| anyhow!("no global named `{field}`"))?;

        let inner = self.inner_mut();
        Ok(Outcome::Ok(vec![global.get(&mut inner.store)]))
    }

    fn get_export(&mut self, module: Option<&str>, name: &str) -> anyhow::Result<Extern> {
        let inner = self.inner_mut();
        if let Some(module) = module {
            return inner
                .linker
                .get(&mut inner.store, module, name)
                .clone()
                .ok_or_else(|| anyhow!("no item named `{}::{}` found", module, name));
        }

        let cur = inner
            .current
            .as_ref()
            .ok_or_else(|| anyhow!("no previous instance found"))?;

        cur.get_export(&mut inner.store, name)
            .ok_or_else(|| anyhow!("no item named `{}` found", name))
    }
}

fn wast_arg_to_val(arg: &WastArgCore) -> anyhow::Result<Val> {
    match arg {
        WastArgCore::I32(v) => Ok(Val::I32(*v)),
        WastArgCore::I64(v) => Ok(Val::I64(*v)),
        WastArgCore::F32(v) => Ok(Val::F32(v.bits)),
        WastArgCore::F64(v) => Ok(Val::F64(v.bits)),
        WastArgCore::V128(v) => Ok(Val::V128(u128::from_le_bytes(v.to_le_bytes()))),
        // WastArgCore::RefNull(HeapType::Abstract {
        //                          ty: AbstractHeapType::Extern,
        //                          shared: false,
        //                      }) => Ok(VMVal::ExternRef(None)),
        // WastArgCore::RefNull(HeapType::Abstract {
        //                          ty: AbstractHeapType::Func,
        //                          shared: false,
        //                      }) => Ok(Value::FuncRef(None)),
        // WastArgCore::RefExtern(x) => Ok(Value::ExternRef(Some(*x))),
        other => bail!("couldn't convert {:?} to a runtime value", other),
    }
}

pub fn match_val(store: &Store<()>, actual: &Val, expected: &WastRetCore) -> anyhow::Result<()> {
    match (actual, expected) {
        (_, WastRetCore::Either(expected)) => {
            for expected in expected {
                if match_val(store, actual, expected).is_ok() {
                    return Ok(());
                }
            }
            match_val(store, actual, &expected[0])
        }

        (Val::I32(a), WastRetCore::I32(b)) => match_int(a, b),
        (Val::I64(a), WastRetCore::I64(b)) => match_int(a, b),

        // Note that these float comparisons are comparing bits, not float
        // values, so we're testing for bit-for-bit equivalence
        (Val::F32(a), WastRetCore::F32(b)) => match_f32(*a, b),
        (Val::F64(a), WastRetCore::F64(b)) => match_f64(*a, b),
        (Val::V128(a), WastRetCore::V128(b)) => match_v128(*a, b),

        // Null references.
        // (
        //     Val::FuncRef(None) | Val::ExternRef(None), /* | Value::AnyRef(None) */
        //     WastRetCore::RefNull(_),
        // )
        // | (Val::ExternRef(None), WastRetCore::RefExtern(None)) => Ok(()),
        //
        // // Null and non-null mismatches.
        // (Val::ExternRef(None), WastRetCore::RefExtern(Some(_))) => {
        //     bail!("expected non-null reference, found null")
        // }
        // (
        //     Val::ExternRef(Some(x)),
        //     WastRetCore::RefNull(Some(HeapType::Abstract {
        //         ty: AbstractHeapType::Extern,
        //         shared: false,
        //     })),
        // ) => {
        //     bail!("expected null externref, found non-null externref of {x}");
        // }
        // (Val::ExternRef(Some(_)) | Val::FuncRef(Some(_)), WastRetCore::RefNull(_)) => {
        //     bail!("expected null, found non-null reference: {actual:?}")
        // }
        //
        // // // Non-null references.
        // (Val::FuncRef(Some(_)), WastRetCore::RefFunc(_)) => Ok(()),
        // (Val::ExternRef(Some(x)), WastRetCore::RefExtern(Some(y))) => {
        //     ensure!(x == y, "expected {} found {}", y, x);
        //     Ok(())
        //     // let x = x
        //     //     .data(store)?
        //     //     .downcast_ref::<u32>()
        //     //     .expect("only u32 externrefs created in wast test suites");
        //     // if x == y {
        //     //     Ok(())
        //     // } else {
        //     //     bail!();
        //     // }
        // }

        // (Value::AnyRef(Some(x)), WastRetCore::RefI31) => {
        //     if x.is_i31(store)? {
        //         Ok(())
        //     } else {
        //         bail!("expected a `(ref i31)`, found {x:?}");
        //     }
        // }
        _ => bail!(
            "don't know how to compare {:?} and {:?} yet",
            actual,
            expected
        ),
    }
}

pub fn match_int<T>(actual: &T, expected: &T) -> anyhow::Result<()>
where
    T: Eq + Display + LowerHex,
{
    if actual == expected {
        Ok(())
    } else {
        bail!(
            "expected {:18} / {0:#018x}\n\
             actual   {:18} / {1:#018x}",
            expected,
            actual
        )
    }
}

pub fn match_f32(actual: u32, expected: &NanPattern<F32>) -> anyhow::Result<()> {
    match expected {
        // Check if an f32 (as u32 bits to avoid possible quieting when moving values in registers, e.g.
        // https://developer.arm.com/documentation/ddi0344/i/neon-and-vfp-programmers-model/modes-of-operation/default-nan-mode?lang=en)
        // is a canonical NaN:
        //  - the sign bit is unspecified,
        //  - the 8-bit exponent is set to all 1s
        //  - the MSB of the payload is set to 1 (a quieted NaN) and all others to 0.
        // See https://webassembly.github.io/spec/core/syntax/values.html#floating-point.
        NanPattern::CanonicalNan => {
            let canon_nan = 0x7fc0_0000;
            if (actual & 0x7fff_ffff) == canon_nan {
                Ok(())
            } else {
                bail!(
                    "expected {:10} / {:#010x}\n\
                     actual   {:10} / {:#010x}",
                    "canon-nan",
                    canon_nan,
                    f32::from_bits(actual),
                    actual,
                )
            }
        }

        // Check if an f32 (as u32, see comments above) is an arithmetic NaN.
        // This is the same as a canonical NaN including that the payload MSB is
        // set to 1, but one or more of the remaining payload bits MAY BE set to
        // 1 (a canonical NaN specifies all 0s). See
        // https://webassembly.github.io/spec/core/syntax/values.html#floating-point.
        NanPattern::ArithmeticNan => {
            const AF32_NAN: u32 = 0x7f80_0000;
            let is_nan = actual & AF32_NAN == AF32_NAN;
            const AF32_PAYLOAD_MSB: u32 = 0x0040_0000;
            let is_msb_set = actual & AF32_PAYLOAD_MSB == AF32_PAYLOAD_MSB;
            if is_nan && is_msb_set {
                Ok(())
            } else {
                bail!(
                    "expected {:>10} / {:>10}\n\
                     actual   {:10} / {:#010x}",
                    "arith-nan",
                    "0x7fc*****",
                    f32::from_bits(actual),
                    actual,
                )
            }
        }
        NanPattern::Value(expected_value) => {
            if actual == expected_value.bits {
                Ok(())
            } else {
                bail!(
                    "expected {:10} / {:#010x}\n\
                     actual   {:10} / {:#010x}",
                    f32::from_bits(expected_value.bits),
                    expected_value.bits,
                    f32::from_bits(actual),
                    actual,
                )
            }
        }
    }
}

pub fn match_f64(actual: u64, expected: &NanPattern<F64>) -> anyhow::Result<()> {
    match expected {
        // Check if an f64 (as u64 bits to avoid possible quieting when moving values in registers, e.g.
        // https://developer.arm.com/documentation/ddi0344/i/neon-and-vfp-programmers-model/modes-of-operation/default-nan-mode?lang=en)
        // is a canonical NaN:
        //  - the sign bit is unspecified,
        //  - the 11-bit exponent is set to all 1s
        //  - the MSB of the payload is set to 1 (a quieted NaN) and all others to 0.
        // See https://webassembly.github.io/spec/core/syntax/values.html#floating-point.
        NanPattern::CanonicalNan => {
            let canon_nan = 0x7ff8_0000_0000_0000;
            if (actual & 0x7fff_ffff_ffff_ffff) == canon_nan {
                Ok(())
            } else {
                bail!(
                    "expected {:18} / {:#018x}\n\
                     actual   {:18} / {:#018x}",
                    "canon-nan",
                    canon_nan,
                    f64::from_bits(actual),
                    actual,
                )
            }
        }

        // Check if an f64 (as u64, see comments above) is an arithmetic NaN. This is the same as a
        // canonical NaN including that the payload MSB is set to 1, but one or more of the remaining
        // payload bits MAY BE set to 1 (a canonical NaN specifies all 0s). See
        // https://webassembly.github.io/spec/core/syntax/values.html#floating-point.
        NanPattern::ArithmeticNan => {
            const AF64_NAN: u64 = 0x7ff0_0000_0000_0000;
            let is_nan = actual & AF64_NAN == AF64_NAN;
            const AF64_PAYLOAD_MSB: u64 = 0x0008_0000_0000_0000;
            let is_msb_set = actual & AF64_PAYLOAD_MSB == AF64_PAYLOAD_MSB;
            if is_nan && is_msb_set {
                Ok(())
            } else {
                bail!(
                    "expected {:>18} / {:>18}\n\
                     actual   {:18} / {:#018x}",
                    "arith-nan",
                    "0x7ff8************",
                    f64::from_bits(actual),
                    actual,
                )
            }
        }
        NanPattern::Value(expected_value) => {
            if actual == expected_value.bits {
                Ok(())
            } else {
                bail!(
                    "expected {:18} / {:#018x}\n\
                     actual   {:18} / {:#018x}",
                    f64::from_bits(expected_value.bits),
                    expected_value.bits,
                    f64::from_bits(actual),
                    actual,
                )
            }
        }
    }
}

fn match_v128(actual: u128, expected: &V128Pattern) -> anyhow::Result<()> {
    match expected {
        V128Pattern::I8x16(expected) => {
            let actual = [
                extract_lane_as_i8(actual, 0),
                extract_lane_as_i8(actual, 1),
                extract_lane_as_i8(actual, 2),
                extract_lane_as_i8(actual, 3),
                extract_lane_as_i8(actual, 4),
                extract_lane_as_i8(actual, 5),
                extract_lane_as_i8(actual, 6),
                extract_lane_as_i8(actual, 7),
                extract_lane_as_i8(actual, 8),
                extract_lane_as_i8(actual, 9),
                extract_lane_as_i8(actual, 10),
                extract_lane_as_i8(actual, 11),
                extract_lane_as_i8(actual, 12),
                extract_lane_as_i8(actual, 13),
                extract_lane_as_i8(actual, 14),
                extract_lane_as_i8(actual, 15),
            ];
            if actual == *expected {
                return Ok(());
            }
            bail!(
                "expected {:4?}\n\
                 actual   {:4?}\n\
                 \n\
                 expected (hex) {0:02x?}\n\
                 actual (hex)   {1:02x?}",
                expected,
                actual,
            )
        }
        V128Pattern::I16x8(expected) => {
            let actual = [
                extract_lane_as_i16(actual, 0),
                extract_lane_as_i16(actual, 1),
                extract_lane_as_i16(actual, 2),
                extract_lane_as_i16(actual, 3),
                extract_lane_as_i16(actual, 4),
                extract_lane_as_i16(actual, 5),
                extract_lane_as_i16(actual, 6),
                extract_lane_as_i16(actual, 7),
            ];
            if actual == *expected {
                return Ok(());
            }
            bail!(
                "expected {:6?}\n\
                 actual   {:6?}\n\
                 \n\
                 expected (hex) {0:04x?}\n\
                 actual (hex)   {1:04x?}",
                expected,
                actual,
            )
        }
        V128Pattern::I32x4(expected) => {
            let actual = [
                extract_lane_as_i32(actual, 0),
                extract_lane_as_i32(actual, 1),
                extract_lane_as_i32(actual, 2),
                extract_lane_as_i32(actual, 3),
            ];
            if actual == *expected {
                return Ok(());
            }
            bail!(
                "expected {:11?}\n\
                 actual   {:11?}\n\
                 \n\
                 expected (hex) {0:08x?}\n\
                 actual (hex)   {1:08x?}",
                expected,
                actual,
            )
        }
        V128Pattern::I64x2(expected) => {
            let actual = [
                extract_lane_as_i64(actual, 0),
                extract_lane_as_i64(actual, 1),
            ];
            if actual == *expected {
                return Ok(());
            }
            bail!(
                "expected {:20?}\n\
                 actual   {:20?}\n\
                 \n\
                 expected (hex) {0:016x?}\n\
                 actual (hex)   {1:016x?}",
                expected,
                actual,
            )
        }
        V128Pattern::F32x4(expected) => {
            for (i, expected) in expected.iter().enumerate() {
                let a = extract_lane_as_i32(actual, i) as u32;
                match_f32(a, expected).with_context(|| format!("difference in lane {i}"))?;
            }
            Ok(())
        }
        V128Pattern::F64x2(expected) => {
            for (i, expected) in expected.iter().enumerate() {
                let a = extract_lane_as_i64(actual, i) as u64;
                match_f64(a, expected).with_context(|| format!("difference in lane {i}"))?;
            }
            Ok(())
        }
    }
}

fn extract_lane_as_i8(bytes: u128, lane: usize) -> i8 {
    (bytes >> (lane * 8)) as i8
}

fn extract_lane_as_i16(bytes: u128, lane: usize) -> i16 {
    (bytes >> (lane * 16)) as i16
}

fn extract_lane_as_i32(bytes: u128, lane: usize) -> i32 {
    (bytes >> (lane * 32)) as i32
}

fn extract_lane_as_i64(bytes: u128, lane: usize) -> i64 {
    (bytes >> (lane * 64)) as i64
}
