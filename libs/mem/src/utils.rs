// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

macro_rules! assert_unsafe_precondition_ {
    ($message:expr, ($($name:ident:$ty:ty = $arg:expr),*$(,)?) => $e:expr $(,)?) => {
        {
            // This check is inlineable, but not by the MIR inliner.
            // The reason for this is that the MIR inliner is in an exceptionally bad position
            // to think about whether or not to inline this. In MIR, this call is gated behind `debug_assertions`,
            // which will codegen to `false` in release builds. Inlining the check would be wasted work in that case and
            // would be bad for compile times.
            //
            // LLVM on the other hand sees the constant branch, so if it's `false`, it can immediately delete it without
            // inlining the check. If it's `true`, it can inline it and get significantly better performance.
            #[inline]
            const fn precondition_check($($name:$ty),*) {
                assert!($e, concat!("unsafe precondition(s) violated: ", $message,
                        "\n\nThis indicates a bug in the program. \
                        This Undefined Behavior check is optional, and cannot be relied on for safety."))
            }

            #[cfg(debug_assertions)]
            precondition_check($($arg,)*);
        }
    };
}

pub(crate) use assert_unsafe_precondition_;

//     pub fn maybe_pick_spot_in(
//         &mut self,
//         layout: Layout,
//         range: &Range<VirtualAddress>,
//     ) -> Option<VirtualAddress> {
//         let spot_count = Self::spots_in_range(layout, range);
//
//         self.candidate_spot_count += spot_count;
//
//         if self.target_index < spot_count {
//             Some(
//                 range
//                     .start
//                     .checked_add(self.target_index << layout.align().ilog2())
//                     .unwrap(),
//             )
//         } else {
//             self.target_index -= spot_count;
//
//             None
//         }
//     }
// }
