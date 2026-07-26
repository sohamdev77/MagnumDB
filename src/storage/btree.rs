//! B+ Tree module
//!
//! Provides the core indexing mechanism based on 4KB pages.
//! Supports B+ Tree splits, node recycling, prefix range scanning, and overflow page chaining for large values.

use super::buffer_pool::BufferPool;
use super::pager::{Page, PageId, PAGE_SIZE, META_ROOT_PAGE_RANGE};

const NODE_TYPE_LEAF: u8 = 0;
const NODE_TYPE_INTERNAL: u8 = 1;

/// Header sizes for bounds checking
const LEAF_HEADER_SIZE: usize = 1 + 2 + 4 + 4; // type + num_records + parent_id + next_leaf = 11
const INTERNAL_HEADER_SIZE: usize = 1 + 2 + 4;  // type + num_keys + parent_id = 7

/// Maximum inline size of a key-value record before spilling to overflow pages.
/// Set conservatively to leave room for header + key length field + value length field.
pub const MAX_INLINE_VAL_SIZE: usize = 2000;

/// Maximum key size. Keys larger than this are rejected.
pub const MAX_KEY_SIZE: usize = 1024;

const OVERFLOW_MAGIC: &[u8; 4] = b"OVRF";

/// In-memory representation of a B+ Tree node.
#[derive(Debug, Clone)]
pub enum Node {
    Leaf(LeafNode),
    Internal(InternalNode),
}

#[derive(Debug, Clone)]
pub struct LeafNode {
    pub parent_page_id: Option<PageId>,
    pub next_leaf: Option<PageId>,
    pub records: Vec<(Vec<u8>, Vec<u8>)>,
}

#[derive(Debug, Clone)]
pub struct InternalNode {
    pub parent_page_id: Option<PageId>,
    pub keys: Vec<Vec<u8>>,
    pub children: Vec<PageId>,
}

impl Node {
    /// Serializes the node into a fixed 4KB page.
    /// Returns an error if the node data exceeds the page size.
    pub fn to_page(&self) -> anyhow::Result<Page> {
        let size = self.encoded_size();
        if size > PAGE_SIZE {
            return Err(anyhow::anyhow!(
                "Node encoded size {} exceeds page size {}. This is a bug — the node should have been split before serialization.",
                size, PAGE_SIZE
            ));
        }

        let mut page = Page::default();
        let mut offset = 0;

        match self {
            Node::Leaf(leaf) => {
                page.data[offset] = NODE_TYPE_LEAF;
                offset += 1;

                let num_records = leaf.records.len() as u16;
                page.data[offset..offset + 2].copy_from_slice(&num_records.to_le_bytes());
                offset += 2;

                let parent_id = leaf.parent_page_id.unwrap_or(u32::MAX);
                page.data[offset..offset + 4].copy_from_slice(&parent_id.to_le_bytes());
                offset += 4;

                let next_id = leaf.next_leaf.unwrap_or(u32::MAX);
                page.data[offset..offset + 4].copy_from_slice(&next_id.to_le_bytes());
                offset += 4;

                for (key, val) in &leaf.records {
                    let klen = key.len() as u32;
                    page.data[offset..offset + 4].copy_from_slice(&klen.to_le_bytes());
                    offset += 4;
                    page.data[offset..offset + key.len()].copy_from_slice(key);
                    offset += key.len();

                    let vlen = val.len() as u32;
                    page.data[offset..offset + 4].copy_from_slice(&vlen.to_le_bytes());
                    offset += 4;
                    page.data[offset..offset + val.len()].copy_from_slice(val);
                    offset += val.len();
                }
            }
            Node::Internal(internal) => {
                page.data[offset] = NODE_TYPE_INTERNAL;
                offset += 1;

                let num_keys = internal.keys.len() as u16;
                page.data[offset..offset + 2].copy_from_slice(&num_keys.to_le_bytes());
                offset += 2;

                let parent_id = internal.parent_page_id.unwrap_or(u32::MAX);
                page.data[offset..offset + 4].copy_from_slice(&parent_id.to_le_bytes());
                offset += 4;

                // Write children (num_keys + 1)
                for child in &internal.children {
                    page.data[offset..offset + 4].copy_from_slice(&child.to_le_bytes());
                    offset += 4;
                }

                // Write keys
                for key in &internal.keys {
                    let klen = key.len() as u32;
                    page.data[offset..offset + 4].copy_from_slice(&klen.to_le_bytes());
                    offset += 4;
                    page.data[offset..offset + key.len()].copy_from_slice(key);
                    offset += key.len();
                }
            }
        }

        Ok(page)
    }

    /// Deserializes a node from a fixed 4KB page.
    pub fn from_page(page: &Page) -> Self {
        let node_type = page.data[0];
        let mut offset = 1;

        let num_elements = u16::from_le_bytes(page.data[offset..offset + 2].try_into().unwrap_or([0; 2]));
        offset += 2;

        let p_id = u32::from_le_bytes(page.data[offset..offset + 4].try_into().unwrap_or([0xFF; 4]));
        let parent_page_id = if p_id == u32::MAX { None } else { Some(p_id) };
        offset += 4;

        if node_type == NODE_TYPE_LEAF {
            let n_id = u32::from_le_bytes(page.data[offset..offset + 4].try_into().unwrap_or([0xFF; 4]));
            let next_leaf = if n_id == u32::MAX { None } else { Some(n_id) };
            offset += 4;

            let mut records = Vec::new();
            for _ in 0..num_elements {
                if offset + 4 > PAGE_SIZE { break; }
                let klen =
                    u32::from_le_bytes(page.data[offset..offset + 4].try_into().unwrap_or([0; 4])) as usize;
                offset += 4;
                if offset + klen > PAGE_SIZE { break; }
                let key = page.data[offset..offset + klen].to_vec();
                offset += klen;

                if offset + 4 > PAGE_SIZE { break; }
                let vlen =
                    u32::from_le_bytes(page.data[offset..offset + 4].try_into().unwrap_or([0; 4])) as usize;
                offset += 4;
                if offset + vlen > PAGE_SIZE { break; }
                let val = page.data[offset..offset + vlen].to_vec();
                offset += vlen;

                records.push((key, val));
            }

            Node::Leaf(LeafNode {
                parent_page_id,
                next_leaf,
                records,
            })
        } else {
            let mut children = Vec::new();
            for _ in 0..=num_elements {
                if offset + 4 > PAGE_SIZE { break; }
                let child = u32::from_le_bytes(page.data[offset..offset + 4].try_into().unwrap_or([0; 4]));
                offset += 4;
                children.push(child);
            }

            let mut keys = Vec::new();
            for _ in 0..num_elements {
                if offset + 4 > PAGE_SIZE { break; }
                let klen =
                    u32::from_le_bytes(page.data[offset..offset + 4].try_into().unwrap_or([0; 4])) as usize;
                offset += 4;
                if offset + klen > PAGE_SIZE { break; }
                let key = page.data[offset..offset + klen].to_vec();
                offset += klen;
                keys.push(key);
            }

            Node::Internal(InternalNode {
                parent_page_id,
                keys,
                children,
            })
        }
    }

    /// Helper to get the total encoded size of the node to check for splits.
    pub fn encoded_size(&self) -> usize {
        match self {
            Node::Leaf(leaf) => {
                let mut size = LEAF_HEADER_SIZE;
                for (k, v) in &leaf.records {
                    size += 4 + k.len() + 4 + v.len();
                }
                size
            }
            Node::Internal(internal) => {
                let mut size = INTERNAL_HEADER_SIZE;
                size += internal.children.len() * 4;
                for k in &internal.keys {
                    size += 4 + k.len();
                }
                size
            }
        }
    }
}

/// The B+ Tree manager.
pub struct BTree {
    buffer_pool: BufferPool,
    root_page_id: PageId,
}

impl BTree {
    pub fn new(mut buffer_pool: BufferPool) -> anyhow::Result<Self> {
        let meta_page = buffer_pool.fetch_page(0)?;
        let stored_root = u32::from_le_bytes(
            meta_page.data[META_ROOT_PAGE_RANGE].try_into().unwrap_or([0; 4]),
        );

        let root_page_id = if stored_root == 0 || stored_root == u32::MAX {
            // Page 1: Root leaf
            let new_page_id = buffer_pool.allocate_page()?;

            let empty_leaf = Node::Leaf(LeafNode {
                parent_page_id: None,
                next_leaf: None,
                records: Vec::new(),
            });
            buffer_pool.write_page(new_page_id, &empty_leaf.to_page()?)?;

            // Write metadata to Page 0
            let mut meta_page = buffer_pool.fetch_page(0)?;
            meta_page.data[META_ROOT_PAGE_RANGE].copy_from_slice(&new_page_id.to_le_bytes());
            buffer_pool.write_page(0, &meta_page)?;

            new_page_id
        } else {
            stored_root
        };

        Ok(Self {
            buffer_pool,
            root_page_id,
        })
    }

    fn persist_root_page_id(&mut self, root_page_id: PageId) -> anyhow::Result<()> {
        let mut meta_page = self.buffer_pool.fetch_page(0)?;
        meta_page.data[META_ROOT_PAGE_RANGE].copy_from_slice(&root_page_id.to_le_bytes());
        self.buffer_pool.write_page(0, &meta_page)?;
        Ok(())
    }

    pub fn flush_all(&mut self) -> anyhow::Result<()> {
        self.buffer_pool.flush_all()
    }

    pub fn flush_and_sync(&mut self) -> anyhow::Result<()> {
        self.buffer_pool.flush_and_sync()
    }

    pub fn sync(&mut self) -> anyhow::Result<()> {
        self.buffer_pool.sync()
    }

    /// Returns a mutable reference to the underlying buffer pool.
    pub fn buffer_pool_mut(&mut self) -> &mut BufferPool {
        &mut self.buffer_pool
    }

    fn read_node(&mut self, page_id: PageId) -> anyhow::Result<Node> {
        let page = self.buffer_pool.fetch_page(page_id)?;
        Ok(Node::from_page(&page))
    }

    fn write_node(&mut self, page_id: PageId, node: &Node) -> anyhow::Result<()> {
        let page = node.to_page()?;
        self.buffer_pool.write_page(page_id, &page)?;
        Ok(())
    }

    /// Stores a large value across overflow pages and returns the overflow record handle.
    fn write_overflow_value(&mut self, value: &[u8]) -> anyhow::Result<Vec<u8>> {
        let chunk_size = PAGE_SIZE - 4;
        let chunks: Vec<&[u8]> = value.chunks(chunk_size).collect();

        let mut prev_page_id: Option<PageId> = None;
        let mut first_page_id: PageId = 0;

        for (i, chunk) in chunks.iter().enumerate().rev() {
            let page_id = self.buffer_pool.allocate_page()?;
            if i == 0 {
                first_page_id = page_id;
            }

            let next_id = prev_page_id.unwrap_or(u32::MAX);
            let mut page = Page::default();
            page.data[0..4].copy_from_slice(&next_id.to_le_bytes());
            page.data[4..4 + chunk.len()].copy_from_slice(chunk);

            self.buffer_pool.write_page(page_id, &page)?;
            prev_page_id = Some(page_id);
        }

        // Format: [OVRF: 4B][Total Length: 4B][First PageId: 4B]
        let mut handle = Vec::with_capacity(12);
        handle.extend_from_slice(OVERFLOW_MAGIC);
        handle.extend_from_slice(&(value.len() as u32).to_le_bytes());
        handle.extend_from_slice(&first_page_id.to_le_bytes());

        Ok(handle)
    }

    /// Reassembles value from overflow pages if it contains an overflow handle.
    fn read_value_resolve_overflow(&mut self, val_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
        if val_bytes.len() == 12 && &val_bytes[0..4] == OVERFLOW_MAGIC {
            let total_len = u32::from_le_bytes(val_bytes[4..8].try_into().unwrap_or([0; 4])) as usize;
            let mut curr_page_id = u32::from_le_bytes(val_bytes[8..12].try_into().unwrap_or([0; 4]));

            let mut result = Vec::with_capacity(total_len);
            let chunk_capacity = PAGE_SIZE - 4;

            while curr_page_id != u32::MAX {
                let page = self.buffer_pool.fetch_page(curr_page_id)?;
                let next_page_id = u32::from_le_bytes(page.data[0..4].try_into().unwrap_or([0xFF; 4]));

                let remaining = total_len - result.len();
                let take_len = remaining.min(chunk_capacity);

                result.extend_from_slice(&page.data[4..4 + take_len]);
                curr_page_id = next_page_id;
            }

            Ok(result)
        } else {
            Ok(val_bytes.to_vec())
        }
    }

    /// Frees overflow pages associated with a record handle.
    fn free_overflow_chain(&mut self, val_bytes: &[u8]) -> anyhow::Result<()> {
        if val_bytes.len() == 12 && &val_bytes[0..4] == OVERFLOW_MAGIC {
            let mut curr_page_id = u32::from_le_bytes(val_bytes[8..12].try_into().unwrap_or([0; 4]));
            while curr_page_id != u32::MAX {
                let page = self.buffer_pool.fetch_page(curr_page_id)?;
                let next_page_id = u32::from_le_bytes(page.data[0..4].try_into().unwrap_or([0xFF; 4]));
                self.buffer_pool.free_page(curr_page_id)?;
                curr_page_id = next_page_id;
            }
        }
        Ok(())
    }

    /// Inserts a key-value pair into the BTree.
    pub fn insert(&mut self, key: &[u8], value: &[u8]) -> anyhow::Result<()> {
        if key.len() > MAX_KEY_SIZE {
            return Err(anyhow::anyhow!(
                "Key size {} exceeds maximum allowed {}",
                key.len(),
                MAX_KEY_SIZE
            ));
        }

        let stored_val = if value.len() > MAX_INLINE_VAL_SIZE {
            self.write_overflow_value(value)?
        } else {
            value.to_vec()
        };

        let root_node = self.read_node(self.root_page_id)?;
        let (leaf_id, mut leaf_node) =
            self.find_leaf_for_insert(self.root_page_id, &root_node, key)?;

        let pos = leaf_node
            .records
            .binary_search_by(|(k, _)| k.as_slice().cmp(key))
            .unwrap_or_else(|e| e);

        if pos < leaf_node.records.len() && leaf_node.records[pos].0 == key {
            // Free previous overflow if replacing
            let old_val = &leaf_node.records[pos].1;
            self.free_overflow_chain(old_val)?;
            leaf_node.records[pos].1 = stored_val;
        } else {
            leaf_node.records.insert(pos, (key.to_vec(), stored_val));
        }

        let updated_leaf = Node::Leaf(leaf_node.clone());

        if updated_leaf.encoded_size() > PAGE_SIZE {
            self.split_leaf_and_insert(leaf_id, leaf_node)?;
        } else {
            self.write_node(leaf_id, &updated_leaf)?;
        }

        Ok(())
    }

    fn find_leaf_for_insert(
        &mut self,
        current_id: PageId,
        current_node: &Node,
        key: &[u8],
    ) -> anyhow::Result<(PageId, LeafNode)> {
        match current_node {
            Node::Leaf(leaf) => Ok((current_id, leaf.clone())),
            Node::Internal(internal) => {
                let mut child_idx = internal.keys.len();
                for (i, k) in internal.keys.iter().enumerate() {
                    if key < k.as_slice() {
                        child_idx = i;
                        break;
                    }
                }
                let child_id = internal.children[child_idx];
                let child_node = self.read_node(child_id)?;
                self.find_leaf_for_insert(child_id, &child_node, key)
            }
        }
    }

    fn split_leaf_and_insert(
        &mut self,
        leaf_id: PageId,
        mut leaf_node: LeafNode,
    ) -> anyhow::Result<()> {
        let mid = leaf_node.records.len() / 2;
        let right_records = leaf_node.records.split_off(mid);
        let split_key = right_records[0].0.clone();

        let new_leaf_id = self.buffer_pool.allocate_page()?;
        let new_leaf_node = LeafNode {
            parent_page_id: leaf_node.parent_page_id,
            next_leaf: leaf_node.next_leaf,
            records: right_records,
        };

        leaf_node.next_leaf = Some(new_leaf_id);

        self.write_node(leaf_id, &Node::Leaf(leaf_node.clone()))?;
        self.write_node(new_leaf_id, &Node::Leaf(new_leaf_node))?;

        if let Some(parent_id) = leaf_node.parent_page_id {
            self.insert_into_internal(parent_id, split_key, new_leaf_id)?;
        } else {
            let new_root_id = self.buffer_pool.allocate_page()?;
            let new_root_node = InternalNode {
                parent_page_id: None,
                keys: vec![split_key],
                children: vec![leaf_id, new_leaf_id],
            };
            self.write_node(new_root_id, &Node::Internal(new_root_node))?;

            let mut left_leaf = leaf_node;
            left_leaf.parent_page_id = Some(new_root_id);
            self.write_node(leaf_id, &Node::Leaf(left_leaf))?;

            let mut right_leaf = self.read_node(new_leaf_id)?;
            if let Node::Leaf(ref mut rl) = right_leaf {
                rl.parent_page_id = Some(new_root_id);
            }
            self.write_node(new_leaf_id, &right_leaf)?;

            self.root_page_id = new_root_id;
            self.persist_root_page_id(new_root_id)?;
        }

        Ok(())
    }

    fn insert_into_internal(
        &mut self,
        parent_id: PageId,
        key: Vec<u8>,
        right_child_id: PageId,
    ) -> anyhow::Result<()> {
        let parent_node = self.read_node(parent_id)?;
        if let Node::Internal(mut internal) = parent_node {
            let pos = internal.keys.binary_search(&key).unwrap_or_else(|e| e);
            internal.keys.insert(pos, key.clone());
            internal.children.insert(pos + 1, right_child_id);

            let updated_internal = Node::Internal(internal.clone());
            if updated_internal.encoded_size() > PAGE_SIZE {
                self.split_internal_and_insert(parent_id, internal)?;
            } else {
                self.write_node(parent_id, &updated_internal)?;
            }
        }
        Ok(())
    }

    fn split_internal_and_insert(
        &mut self,
        internal_id: PageId,
        mut internal_node: InternalNode,
    ) -> anyhow::Result<()> {
        let mid = internal_node.keys.len() / 2;
        let push_up_key = internal_node.keys.remove(mid);
        let right_keys = internal_node.keys.split_off(mid);
        let right_children = internal_node.children.split_off(mid + 1);

        let new_internal_id = self.buffer_pool.allocate_page()?;
        let new_internal_node = InternalNode {
            parent_page_id: internal_node.parent_page_id,
            keys: right_keys,
            children: right_children.clone(),
        };

        self.write_node(internal_id, &Node::Internal(internal_node.clone()))?;
        self.write_node(new_internal_id, &Node::Internal(new_internal_node))?;

        for child_id in right_children {
            let mut child = self.read_node(child_id)?;
            match &mut child {
                Node::Leaf(l) => l.parent_page_id = Some(new_internal_id),
                Node::Internal(i) => i.parent_page_id = Some(new_internal_id),
            }
            self.write_node(child_id, &child)?;
        }

        if let Some(parent_id) = internal_node.parent_page_id {
            self.insert_into_internal(parent_id, push_up_key, new_internal_id)?;
        } else {
            let new_root_id = self.buffer_pool.allocate_page()?;
            let new_root_node = InternalNode {
                parent_page_id: None,
                keys: vec![push_up_key],
                children: vec![internal_id, new_internal_id],
            };
            self.write_node(new_root_id, &Node::Internal(new_root_node))?;

            let mut left_internal = internal_node;
            left_internal.parent_page_id = Some(new_root_id);
            self.write_node(internal_id, &Node::Internal(left_internal))?;

            let mut right_internal = self.read_node(new_internal_id)?;
            if let Node::Internal(ref mut ri) = right_internal {
                ri.parent_page_id = Some(new_root_id);
            }
            self.write_node(new_internal_id, &right_internal)?;

            self.root_page_id = new_root_id;
            self.persist_root_page_id(new_root_id)?;
        }

        Ok(())
    }

    /// Retrieves a value by key.
    pub fn search(&mut self, key: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        let root_node = self.read_node(self.root_page_id)?;
        let (_, leaf_node) = self.find_leaf_for_insert(self.root_page_id, &root_node, key)?;

        if let Ok(pos) = leaf_node
            .records
            .binary_search_by(|(k, _)| k.as_slice().cmp(key))
        {
            let raw_val = &leaf_node.records[pos].1;
            let resolved = self.read_value_resolve_overflow(raw_val)?;
            Ok(Some(resolved))
        } else {
            Ok(None)
        }
    }

    /// Scans all records in the BTree (table scan).
    pub fn scan(&mut self) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        self.scan_prefix(b"")
    }

    /// Scans only records matching a specific key prefix.
    pub fn scan_prefix(&mut self, prefix: &[u8]) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut results = Vec::new();

        let root_node = self.read_node(self.root_page_id)?;
        let (_current_id, mut current_leaf) =
            self.find_leaf_for_insert(self.root_page_id, &root_node, prefix)?;

        'outer: loop {
            for rec in &current_leaf.records {
                if prefix.is_empty() || rec.0.starts_with(prefix) {
                    let resolved_val = self.read_value_resolve_overflow(&rec.1)?;
                    results.push((rec.0.clone(), resolved_val));
                } else if !prefix.is_empty() && rec.0.as_slice() > prefix {
                    // Beyond prefix bound in sorted leaf
                    if !rec.0.starts_with(prefix) {
                        break 'outer;
                    }
                }
            }

            if let Some(next_id) = current_leaf.next_leaf {
                let next_node = self.read_node(next_id)?;
                if let Node::Leaf(next_leaf) = next_node {
                    current_leaf = next_leaf;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        Ok(results)
    }

    /// Deletes a key from the BTree.
    /// If the leaf becomes empty after deletion, the page is freed and sibling pointers are updated.
    pub fn delete(&mut self, key: &[u8]) -> anyhow::Result<()> {
        let root_node = self.read_node(self.root_page_id)?;
        let (leaf_id, mut leaf_node) =
            self.find_leaf_for_insert(self.root_page_id, &root_node, key)?;

        if let Ok(pos) = leaf_node
            .records
            .binary_search_by(|(k, _)| k.as_slice().cmp(key))
        {
            let removed = leaf_node.records.remove(pos);
            self.free_overflow_chain(&removed.1)?;

            // If the leaf is empty and it's not the root, free the page
            if leaf_node.records.is_empty() && leaf_id != self.root_page_id {
                // Update the previous leaf's next_leaf pointer to skip this leaf.
                // We do a scan from the leftmost leaf to find the previous sibling.
                self.unlink_empty_leaf(leaf_id, &leaf_node)?;
            } else {
                self.write_node(leaf_id, &Node::Leaf(leaf_node))?;
            }
        }

        Ok(())
    }

    /// Unlinks an empty leaf from the sibling chain and frees its page.
    fn unlink_empty_leaf(&mut self, leaf_id: PageId, leaf_node: &LeafNode) -> anyhow::Result<()> {
        // Find the leftmost leaf by traversing from root
        let leftmost_leaf_id = self.find_leftmost_leaf(self.root_page_id)?;

        if leftmost_leaf_id == leaf_id {
            // This is the leftmost leaf, just write it empty (don't free root-adjacent leaves
            // to avoid complex parent pointer updates)
            self.write_node(leaf_id, &Node::Leaf(leaf_node.clone()))?;
            return Ok(());
        }

        // Walk the sibling chain to find the previous leaf
        let mut prev_id = leftmost_leaf_id;
        loop {
            let prev_node = self.read_node(prev_id)?;
            if let Node::Leaf(mut prev_leaf) = prev_node {
                if prev_leaf.next_leaf == Some(leaf_id) {
                    // Found the previous sibling — update its pointer
                    prev_leaf.next_leaf = leaf_node.next_leaf;
                    self.write_node(prev_id, &Node::Leaf(prev_leaf))?;
                    self.buffer_pool.free_page(leaf_id)?;
                    return Ok(());
                }
                if let Some(next) = prev_leaf.next_leaf {
                    prev_id = next;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        // Fallback: couldn't find prev sibling, just write the empty leaf
        self.write_node(leaf_id, &Node::Leaf(leaf_node.clone()))?;
        Ok(())
    }

    /// Finds the leftmost leaf page by always taking the first child from root.
    fn find_leftmost_leaf(&mut self, page_id: PageId) -> anyhow::Result<PageId> {
        let node = self.read_node(page_id)?;
        match node {
            Node::Leaf(_) => Ok(page_id),
            Node::Internal(internal) => {
                if internal.children.is_empty() {
                    Ok(page_id)
                } else {
                    self.find_leftmost_leaf(internal.children[0])
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::pager::Pager;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};
    use std::collections::BTreeMap;
    use tempfile::NamedTempFile;

    fn setup_btree() -> BTree {
        let temp_file = NamedTempFile::new().unwrap();
        let pager = Pager::open(temp_file.path()).unwrap();
        let buffer_pool = BufferPool::new(pager, 100);
        BTree::new(buffer_pool).unwrap()
    }

    #[test]
    fn test_btree_insert_get_delete() {
        let mut btree = setup_btree();

        btree.insert(b"key1", b"value1").unwrap();
        btree.insert(b"key2", b"value2").unwrap();

        assert_eq!(btree.search(b"key1").unwrap().unwrap(), b"value1");
        assert_eq!(btree.search(b"key2").unwrap().unwrap(), b"value2");
        assert!(btree.search(b"key3").unwrap().is_none());

        btree.delete(b"key1").unwrap();
        assert!(btree.search(b"key1").unwrap().is_none());
        assert_eq!(btree.search(b"key2").unwrap().unwrap(), b"value2");
    }

    #[test]
    fn test_btree_overflow_large_record() {
        let mut btree = setup_btree();
        let key = b"large_key";
        let val = vec![42u8; 10000]; // 10KB payload

        btree.insert(key, &val).unwrap();
        let read_val = btree.search(key).unwrap().unwrap();
        assert_eq!(read_val, val);

        btree.delete(key).unwrap();
        assert!(btree.search(key).unwrap().is_none());
    }

    #[test]
    fn test_btree_rejects_oversized_key() {
        let mut btree = setup_btree();
        let big_key = vec![b'x'; MAX_KEY_SIZE + 1];
        let result = btree.insert(&big_key, b"val");
        assert!(result.is_err());
    }

    #[test]
    fn test_btree_prefix_scan() {
        let mut btree = setup_btree();
        btree.insert(b"user:1", b"alice").unwrap();
        btree.insert(b"user:2", b"bob").unwrap();
        btree.insert(b"post:1", b"hello").unwrap();

        let users = btree.scan_prefix(b"user:").unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].0, b"user:1");
        assert_eq!(users[1].0, b"user:2");

        let posts = btree.scan_prefix(b"post:").unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].0, b"post:1");
    }

    #[test]
    fn test_btree_split_behavior() {
        let mut btree = setup_btree();

        for i in 0..300 {
            let key = format!("key{:03}", i);
            let val = format!("val{:03}", i);
            btree.insert(key.as_bytes(), val.as_bytes()).unwrap();
        }

        for i in 0..300 {
            let key = format!("key{:03}", i);
            let val = format!("val{:03}", i);
            assert_eq!(
                btree.search(key.as_bytes()).unwrap().unwrap(),
                val.as_bytes()
            );
        }

        let root = btree.read_node(btree.root_page_id).unwrap();
        assert!(matches!(root, Node::Internal(_)));
    }

    #[test]
    fn test_btree_randomized_sequence() {
        let mut btree = setup_btree();
        let mut ref_map = BTreeMap::new();
        let mut rng = StdRng::seed_from_u64(42);

        for _ in 0..1000 {
            let op = rng.gen_range(0..100);
            let key = format!("k{}", rng.gen_range(0..200));

            if op < 70 {
                let val = format!("v{}", rng.gen_range(0..1000));
                btree.insert(key.as_bytes(), val.as_bytes()).unwrap();
                ref_map.insert(key, val);
            } else if op < 85 {
                btree.delete(key.as_bytes()).unwrap();
                ref_map.remove(&key);
            } else {
                let res = btree.search(key.as_bytes()).unwrap();
                let ref_val = ref_map.get(&key);
                match ref_val {
                    Some(v) => assert_eq!(res.unwrap(), v.as_bytes()),
                    None => assert!(res.is_none()),
                }
            }
        }

        for (k, v) in ref_map {
            assert_eq!(btree.search(k.as_bytes()).unwrap().unwrap(), v.as_bytes());
        }
    }
}
