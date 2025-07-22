// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::ptr::NonNull;

use crate::utils::Side;
use crate::{Linked, WAVLTree};

pub struct Dot<'a, T>
where
    T: Linked + ?Sized,
{
    pub(crate) tree: &'a WAVLTree<T>,
}

impl<T> Dot<'_, T>
where
    T: Linked + fmt::Debug + ?Sized,
{
    #[allow(
        clippy::only_used_in_recursion,
        reason = "need to ensure tree is borrowed for the entire time we operate on it"
    )]
    fn node_fmt(&self, f: &mut fmt::Formatter, node: NonNull<T>) -> fmt::Result {
        unsafe {
            let node_links = T::links(node).as_ref();

            let id = node.as_ptr().cast::<u8>() as usize;
            #[cfg(debug_assertions)]
            writeln!(
                f,
                r#"{id} [label="node = {node:#?} rank = {rank}, rank_parity = {rank_parity}"];"#,
                node = node.as_ref(),
                rank = node_links.rank(),
                rank_parity = node_links.rank_parity(),
            )?;
            #[cfg(not(debug_assertions))]
            writeln!(
                f,
                r#"{id} [label="node = {:#?} rank_parity = {}"];"#,
                node.as_ref(),
                node_links.rank_parity(),
            )?;

            if let Some(up) = node_links.parent() {
                writeln!(
                    f,
                    r#"{id} -> {} [label="up"];"#,
                    up.as_ptr().cast::<u8>() as usize
                )?;
            }

            let mut print_side = |side: Side| -> fmt::Result {
                if let Some(child) = node_links.child(side) {
                    writeln!(
                        f,
                        r#"{id} -> {} [label="{side}"];"#,
                        child.as_ptr().cast::<u8>() as usize,
                    )?;

                    self.node_fmt(f, child)?;
                }
                Ok(())
            };
            print_side(Side::Left)?;
            print_side(Side::Right)?;
        }

        Ok(())
    }
}

impl<T> fmt::Display for Dot<'_, T>
where
    T: Linked + fmt::Debug + ?Sized,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "digraph {{")?;
        if let Some(root) = self.tree.root {
            self.node_fmt(f, root)?;
        }
        writeln!(f, "}}")?;

        Ok(())
    }
}

impl<T> fmt::Debug for Dot<'_, T>
where
    T: Linked + fmt::Debug + ?Sized,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "digraph {{")?;
        if let Some(root) = self.tree.root {
            self.node_fmt(f, root)?;
        }
        writeln!(f, "}}")?;

        Ok(())
    }
}
