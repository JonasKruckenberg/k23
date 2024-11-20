use crate::utils::Side;
use crate::Linked;
use crate::WAVLTree;
use core::fmt;
use core::ptr::NonNull;

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
        f.write_str("digraph {")?;

        unsafe {
            let node_links = T::links(node).as_ref();

            let id = node.as_ptr().cast::<u8>() as usize;
            #[cfg(debug_assertions)]
            f.write_fmt(format_args!(
                r#"{id} [label="node = {node:?} rank = {rank}, rank_parity = {rank_parity}"];"#,
                node = node.as_ref(),
                rank = node_links.rank(),
                rank_parity = node_links.rank_parity(),
            ))?;
            #[cfg(not(debug_assertions))]
            f.write_fmt(format_args!(
                r#"{id} [label="node = {:?} rank_parity = {}"];"#,
                node.as_ref(),
                node_links.rank_parity(),
            ))?;

            if let Some(up) = node_links.parent() {
                f.write_fmt(format_args!(
                    r#"{id} -> {} [label="up"];"#,
                    up.as_ptr().cast::<u8>() as usize
                ))?;
            }

            let mut print_side = |side: Side| -> fmt::Result {
                if let Some(child) = node_links.child(side) {
                    f.write_fmt(format_args!(
                        r#"{id} -> {} [label="{side}"];"#,
                        child.as_ptr().cast::<u8>() as usize,
                    ))?;
                    self.node_fmt(f, child)?;
                }
                Ok(())
            };
            print_side(Side::Left)?;
            print_side(Side::Right)?;
        }

        f.write_str("}")
    }
}

impl<T> fmt::Display for Dot<'_, T>
where
    T: Linked + fmt::Debug + ?Sized,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(root) = self.tree.root {
            self.node_fmt(f, root)?;
        }

        Ok(())
    }
}

impl<T> fmt::Debug for Dot<'_, T>
where
    T: Linked + fmt::Debug + ?Sized,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(root) = self.tree.root {
            self.node_fmt(f, root)?;
        }

        Ok(())
    }
}
