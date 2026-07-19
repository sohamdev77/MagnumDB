//! B+ Tree module
//!
//! Provides the core indexing mechanism based on 4KB pages.

use super::buffer_pool::BufferPool;
use super::pager::{Page, PageId, PAGE_SIZE};

const NODE_TYPE_LEAF: u8 = 0;
const NODE_TYPE_INTERNAL: u8 = 1;

/// Maximum size of a key-value pair to fit in a page.
pub const MAX_RECORD_SIZE: usize = 4000;

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
    pub fn to_page(&self) -> Page {
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

        page
    }

    /// Deserializes a node from a fixed 4KB page.
    pub fn from_page(page: &Page) -> Self {
        let node_type = page.data[0];
        let mut offset = 1;

        let num_elements = u16::from_le_bytes(page.data[offset..offset + 2].try_into().unwrap());
        offset += 2;

        let p_id = u32::from_le_bytes(page.data[offset..offset + 4].try_into().unwrap());
        let parent_page_id = if p_id == u32::MAX { None } else { Some(p_id) };
        offset += 4;

        if node_type == NODE_TYPE_LEAF {
            let n_id = u32::from_le_bytes(page.data[offset..offset + 4].try_into().unwrap());
            let next_leaf = if n_id == u32::MAX { None } else { Some(n_id) };
            offset += 4;

            let mut records = Vec::new();
            for _ in 0..num_elements {
                let klen =
                    u32::from_le_bytes(page.data[offset..offset + 4].try_into().unwrap()) as usize;
                offset += 4;
                let key = page.data[offset..offset + klen].to_vec();
                offset += klen;

                let vlen =
                    u32::from_le_bytes(page.data[offset..offset + 4].try_into().unwrap()) as usize;
                offset += 4;
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
                let child = u32::from_le_bytes(page.data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                children.push(child);
            }

            let mut keys = Vec::new();
            for _ in 0..num_elements {
                let klen =
                    u32::from_le_bytes(page.data[offset..offset + 4].try_into().unwrap()) as usize;
                offset += 4;
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
                let mut size = 1 + 2 + 4 + 4; // header
                for (k, v) in &leaf.records {
                    size += 4 + k.len() + 4 + v.len();
                }
                size
            }
            Node::Internal(internal) => {
                let mut size = 1 + 2 + 4; // header
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
        let root_page_id = if buffer_pool.get_num_pages() == 0 {
            // Page 0: Metadata
            let meta_id = buffer_pool.allocate_page()?;
            // Page 1: Root leaf
            let new_page_id = buffer_pool.allocate_page()?;

            let empty_leaf = Node::Leaf(LeafNode {
                parent_page_id: None,
                next_leaf: None,
                records: Vec::new(),
            });
            buffer_pool.write_page(new_page_id, &empty_leaf.to_page())?;

            // Write metadata
            let mut meta_page = super::pager::Page::default();
            meta_page.data[0..4].copy_from_slice(&new_page_id.to_le_bytes());
            buffer_pool.write_page(meta_id, &meta_page)?;

            new_page_id
        } else {
            let meta_page = buffer_pool.fetch_page(0)?;
            u32::from_le_bytes(meta_page.data[0..4].try_into().unwrap())
        };

        Ok(Self {
            buffer_pool,
            root_page_id,
        })
    }

    fn persist_root_page_id(&mut self, root_page_id: PageId) -> anyhow::Result<()> {
        let mut meta_page = self.buffer_pool.fetch_page(0)?;
        meta_page.data[0..4].copy_from_slice(&root_page_id.to_le_bytes());
        self.buffer_pool.write_page(0, &meta_page)?;
        Ok(())
    }

    pub fn flush_all(&mut self) -> anyhow::Result<()> {
        self.buffer_pool.flush_all()
    }

    pub fn sync(&mut self) -> anyhow::Result<()> {
        self.buffer_pool.sync()
    }

    fn read_node(&mut self, page_id: PageId) -> anyhow::Result<Node> {
        let page = self.buffer_pool.fetch_page(page_id)?;
        Ok(Node::from_page(&page))
    }

    fn write_node(&mut self, page_id: PageId, node: &Node) -> anyhow::Result<()> {
        let page = node.to_page();
        self.buffer_pool.write_page(page_id, &page)?;
        Ok(())
    }

    /// Inserts a key-value pair into the BTree.
    pub fn insert(&mut self, key: &[u8], value: &[u8]) -> anyhow::Result<()> {
        if key.len() + value.len() > MAX_RECORD_SIZE {
            return Err(anyhow::anyhow!("Record size exceeds maximum allowed limit"));
        }

        let root_node = self.read_node(self.root_page_id)?;

        let (leaf_id, mut leaf_node) =
            self.find_leaf_for_insert(self.root_page_id, &root_node, key)?;

        // Insert into the leaf node
        let pos = leaf_node
            .records
            .binary_search_by(|(k, _)| k.as_slice().cmp(key))
            .unwrap_or_else(|e| e);
        if pos < leaf_node.records.len() && leaf_node.records[pos].0 == key {
            // Update existing
            leaf_node.records[pos].1 = value.to_vec();
        } else {
            // Insert new
            leaf_node
                .records
                .insert(pos, (key.to_vec(), value.to_vec()));
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
        // Split records in half
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
            // Split the root!
            let new_root_id = self.buffer_pool.allocate_page()?;
            let new_root_node = InternalNode {
                parent_page_id: None,
                keys: vec![split_key],
                children: vec![leaf_id, new_leaf_id],
            };
            self.write_node(new_root_id, &Node::Internal(new_root_node))?;

            // Update children's parent pointers
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

        // Update parent pointers of right_children
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
            // Split the root!
            let new_root_id = self.buffer_pool.allocate_page()?;
            let new_root_node = InternalNode {
                parent_page_id: None,
                keys: vec![push_up_key],
                children: vec![internal_id, new_internal_id],
            };
            self.write_node(new_root_id, &Node::Internal(new_root_node))?;

            // Update children's parent pointers
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
            Ok(Some(leaf_node.records[pos].1.clone()))
        } else {
            Ok(None)
        }
    }

    /// Scans all records in the BTree (table scan).
    pub fn scan(&mut self) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut results = Vec::new();

        // Find leftmost leaf
        let mut current_id = self.root_page_id;
        let mut current_node = self.read_node(current_id)?;

        while let Node::Internal(internal) = current_node {
            current_id = internal.children[0];
            current_node = self.read_node(current_id)?;
        }

        // Now we are at the leftmost leaf. Traverse next_leaf pointers.
        while let Node::Leaf(leaf) = current_node {
            for rec in &leaf.records {
                results.push(rec.clone());
            }

            if let Some(next_id) = leaf.next_leaf {
                current_id = next_id;
                current_node = self.read_node(current_id)?;
            } else {
                break;
            }
        }

        Ok(results)
    }

    /// Deletes a key from the BTree.
    pub fn delete(&mut self, key: &[u8]) -> anyhow::Result<()> {
        let root_node = self.read_node(self.root_page_id)?;
        let (leaf_id, mut leaf_node) =
            self.find_leaf_for_insert(self.root_page_id, &root_node, key)?;

        if let Ok(pos) = leaf_node
            .records
            .binary_search_by(|(k, _)| k.as_slice().cmp(key))
        {
            leaf_node.records.remove(pos);
            self.write_node(leaf_id, &Node::Leaf(leaf_node))?;
        }

        Ok(())
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

        // Insert
        btree.insert(b"key1", b"value1").unwrap();
        btree.insert(b"key2", b"value2").unwrap();

        // Get
        assert_eq!(btree.search(b"key1").unwrap().unwrap(), b"value1");
        assert_eq!(btree.search(b"key2").unwrap().unwrap(), b"value2");
        assert!(btree.search(b"key3").unwrap().is_none());

        // Delete
        btree.delete(b"key1").unwrap();
        assert!(btree.search(b"key1").unwrap().is_none());
        assert_eq!(btree.search(b"key2").unwrap().unwrap(), b"value2");
    }

    #[test]
    fn test_btree_split_behavior() {
        let mut btree = setup_btree();

        // Insert enough records to force a leaf split.
        // A page is 4096 bytes. Each record here is ~20 bytes.
        // 300 records = 6000 bytes, which forces a split.
        for i in 0..300 {
            let key = format!("key{:03}", i);
            let val = format!("val{:03}", i);
            btree.insert(key.as_bytes(), val.as_bytes()).unwrap();
        }

        // Verify all records exist
        for i in 0..300 {
            let key = format!("key{:03}", i);
            let val = format!("val{:03}", i);
            assert_eq!(
                btree.search(key.as_bytes()).unwrap().unwrap(),
                val.as_bytes()
            );
        }

        // Ensure root is an internal node now
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
                // 70% chance to insert
                let val = format!("v{}", rng.gen_range(0..1000));
                btree.insert(key.as_bytes(), val.as_bytes()).unwrap();
                ref_map.insert(key, val);
            } else if op < 85 {
                // 15% chance to delete
                btree.delete(key.as_bytes()).unwrap();
                ref_map.remove(&key);
            } else {
                // 15% chance to get
                let res = btree.search(key.as_bytes()).unwrap();
                let ref_val = ref_map.get(&key);
                match ref_val {
                    Some(v) => assert_eq!(res.unwrap(), v.as_bytes()),
                    None => assert!(res.is_none()),
                }
            }
        }

        // Final verification
        for (k, v) in ref_map {
            assert_eq!(btree.search(k.as_bytes()).unwrap().unwrap(), v.as_bytes());
        }
    }

    #[test]
    fn test_btree_root_persistence() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();

        {
            let pager = Pager::open(&path).unwrap();
            let buffer_pool = BufferPool::new(pager, 100);
            let mut btree = BTree::new(buffer_pool).unwrap();

            // Insert enough to split root
            for i in 0..300 {
                let key = format!("key{:03}", i);
                let val = format!("val{:03}", i);
                btree.insert(key.as_bytes(), val.as_bytes()).unwrap();
            }
            btree.flush_all().unwrap();
            btree.sync().unwrap();
        } // BTree closed

        {
            // Reopen and verify
            let pager = Pager::open(&path).unwrap();
            let buffer_pool = BufferPool::new(pager, 100);
            let mut btree = BTree::new(buffer_pool).unwrap();

            for i in 0..300 {
                let key = format!("key{:03}", i);
                let val = format!("val{:03}", i);
                assert_eq!(
                    btree.search(key.as_bytes()).unwrap().unwrap(),
                    val.as_bytes()
                );
            }
        }
    }

    #[test]
    fn test_btree_oversized_record() {
        let mut btree = setup_btree();
        let key = b"large_key";
        let val = vec![0u8; 4000]; // Too big

        let res = btree.insert(key, &val);
        assert!(res.is_err());
    }
}
