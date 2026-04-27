// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

/// Returns the default register rule for the given register on this architecture.
pub fn default_register_rule_for(_reg: Register) -> RegisterRule<usize> {
    // As far as I can tell x86_64 has no special requirements
    RegisterRule::Undefined
}
