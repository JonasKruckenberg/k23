// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::str::FromStr;

use crate::backtrace::BacktraceStyle;
use crate::device_tree::DeviceTree;
use crate::tracing::Filter;

pub fn parse(devtree: &DeviceTree) -> crate::Result<Bootargs> {
    let chosen = devtree.find_by_path("/chosen").unwrap();
    let Some(prop) = chosen.property("bootargs") else {
        return Ok(Bootargs::default());
    };

    Bootargs::from_str(prop.as_str()?)
}

#[derive(Default)]
pub struct Bootargs {
    pub log: Filter,
    pub backtrace: BacktraceStyle,
}

impl FromStr for Bootargs {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut log = None;
        let mut backtrace = None;

        let parts = s.trim().split(';');
        for part in parts {
            if let Some(current) = part.strip_prefix("log=") {
                log = Some(Filter::from_str(current)?);
            }

            if let Some(current) = part.strip_prefix("backtrace=") {
                backtrace = Some(BacktraceStyle::from_str(current)?);
            }
        }

        Ok(Self {
            log: log.unwrap_or_default(),
            backtrace: backtrace.unwrap_or_default(),
        })
    }
}
