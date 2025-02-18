// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::device_tree::DeviceTree;
use crate::error::Error;
use crate::tracing::Filter;
use core::str::FromStr;

pub fn parse(devtree: &DeviceTree) -> Result<Cmdline, Error> {
    let chosen = devtree.find_by_path("/chosen").unwrap();
    let Some(prop) = chosen.property("bootargs") else {
        return Ok(Cmdline::default());
    };

    Cmdline::from_str(prop.as_str()?)
}

#[derive(Default)]
pub struct Cmdline {
    pub log: Filter,
}

impl FromStr for Cmdline {
    type Err = Error;

    #[expect(tail_expr_drop_order, reason = "")]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut log = None;

        let s = s.trim();
        if let Some(current) = s.strip_prefix("log=") {
            log = Some(Filter::from_str(current).unwrap());
        }

        Ok(Self {
            log: log.unwrap_or_default(),
        })
    }
}
