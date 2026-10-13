use crate::key_notation::KeyStroke;
use mlua::Function;
use std::collections::HashMap;

#[derive(Clone)]
pub struct Binding {
    pub callback: Function,
    pub desc: Option<String>,
}

#[derive(Clone, Default)]
pub struct TrieNode {
    pub children: HashMap<KeyStroke, TrieNode>,
    pub binding: Option<Binding>,
}

#[derive(Clone, Default)]
pub struct BindingTrie {
    root: TrieNode,
}

impl BindingTrie {
    pub fn bind(&mut self, sequence: &[KeyStroke], binding: Binding) -> Result<(), String> {
        let mut node = &mut self.root;
        for key in sequence {
            node = node.children.entry(*key).or_default();
        }
        node.binding = Some(binding);
        Ok(())
    }

    pub fn unbind(&mut self, sequence: &[KeyStroke]) -> bool {
        fn remove(node: &mut TrieNode, sequence: &[KeyStroke]) -> bool {
            let Some((key, rest)) = sequence.split_first() else {
                return node.binding.take().is_some();
            };
            let Some(child) = node.children.get_mut(key) else {
                return false;
            };
            let removed = remove(child, rest);
            if child.binding.is_none() && child.children.is_empty() {
                node.children.remove(key);
            }
            removed
        }
        remove(&mut self.root, sequence)
    }

    pub fn node_for(&self, sequence: &[KeyStroke]) -> Option<&TrieNode> {
        let mut node = &self.root;
        for key in sequence {
            node = node.children.get(key)?;
        }
        Some(node)
    }

    pub fn completions(&self, prefix: &[KeyStroke]) -> Vec<(KeyStroke, Option<String>)> {
        let Some(node) = self.node_for(prefix) else {
            return Vec::new();
        };
        let mut completions: Vec<_> = node
            .children
            .iter()
            .map(|(key, child)| {
                (
                    *key,
                    child
                        .binding
                        .as_ref()
                        .and_then(|binding| binding.desc.clone()),
                )
            })
            .collect();
        completions.sort_by_key(|(key, _)| *key);
        completions
    }

    #[cfg(test)]
    pub fn key_count(&self) -> usize {
        fn count(node: &TrieNode) -> usize {
            usize::from(node.binding.is_some()) + node.children.values().map(count).sum::<usize>()
        }
        count(&self.root)
    }
}
