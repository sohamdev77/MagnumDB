use super::pager::{Page, PageId, Pager};
use lru::LruCache;
use std::num::NonZeroUsize;

/// The Buffer Pool Manager is responsible for fetching pages from the Pager,
/// caching them in memory, and evicting the least recently used pages back to disk.
pub struct BufferPool {
    pager: Pager,
    /// Caches PageId -> (Page, is_dirty)
    cache: LruCache<PageId, (Page, bool)>, 
}

impl BufferPool {
    /// Creates a new BufferPool wrapping a Pager with a specific capacity limit.
    pub fn new(pager: Pager, capacity: usize) -> Self {
        Self {
            pager,
            cache: LruCache::new(NonZeroUsize::new(capacity).unwrap()),
        }
    }

    /// Fetches a page from the buffer pool or reads it from disk if not present.
    pub fn fetch_page(&mut self, page_id: PageId) -> anyhow::Result<Page> {
        if let Some((page, _)) = self.cache.get(&page_id) {
            return Ok(page.clone());
        }

        let page = self.pager.read_page(page_id)?;
        
        self.put_and_evict(page_id, page.clone(), false)?;

        Ok(page)
    }

    /// Writes a page to the buffer pool, marking it as dirty.
    pub fn write_page(&mut self, page_id: PageId, page: &Page) -> anyhow::Result<()> {
        self.put_and_evict(page_id, page.clone(), true)?;
        Ok(())
    }

    /// Allocates a new page on disk and returns its ID.
    pub fn allocate_page(&mut self) -> anyhow::Result<PageId> {
        let page_id = self.pager.allocate_page()?;
        Ok(page_id)
    }

    /// Helper function to put a page in the cache and evict the oldest if full.
    fn put_and_evict(&mut self, page_id: PageId, page: Page, is_dirty: bool) -> anyhow::Result<()> {
        if self.cache.len() == self.cache.cap().get() {
            // Need to evict if we are inserting a new page (not updating an existing one)
            if !self.cache.contains(&page_id) {
                if let Some((evict_id, (evict_page, evict_dirty))) = self.cache.pop_lru() {
                    if evict_dirty {
                        self.pager.write_page(evict_id, &evict_page)?;
                    }
                }
            }
        }
        
        // If we already have it, we just update it. `put` updates the value and moves to MRU.
        if let Some((_, curr_dirty)) = self.cache.get(&page_id) {
            // Keep it dirty if it was already dirty
            let new_dirty = is_dirty || *curr_dirty;
            self.cache.put(page_id, (page, new_dirty));
        } else {
            self.cache.put(page_id, (page, is_dirty));
        }
        
        Ok(())
    }

    /// Flushes all dirty pages back to disk.
    pub fn flush_all(&mut self) -> anyhow::Result<()> {
        let mut to_flush = Vec::new();
        for (page_id, (page, dirty)) in self.cache.iter() {
            if *dirty {
                to_flush.push((*page_id, page.clone()));
            }
        }
        
        for (page_id, page) in to_flush {
            self.pager.write_page(page_id, &page)?;
            if let Some(entry) = self.cache.get_mut(&page_id) {
                entry.1 = false;
            }
        }
        
        Ok(())
    }
    
    /// Returns the total number of allocated pages on disk.
    pub fn get_num_pages(&self) -> u32 {
        self.pager.num_pages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_buffer_pool_eviction() -> anyhow::Result<()> {
        let temp_file = NamedTempFile::new()?;
        let path = temp_file.path().to_path_buf();
        
        let pager = Pager::open(&path)?;
        // Create pool with capacity of exactly 2 pages
        let mut pool = BufferPool::new(pager, 2);

        // Allocate 3 pages
        let id0 = pool.allocate_page()?;
        let id1 = pool.allocate_page()?;
        let id2 = pool.allocate_page()?;

        // Write page 0
        let mut page0 = Page::default();
        page0.data[0] = 100;
        pool.write_page(id0, &page0)?;

        // Write page 1
        let mut page1 = Page::default();
        page1.data[0] = 101;
        pool.write_page(id1, &page1)?;

        // At this point, cache has id0 and id1.
        // Write page 2 -> this should evict id0 because it's the LRU
        let mut page2 = Page::default();
        page2.data[0] = 102;
        pool.write_page(id2, &page2)?;

        // Now cache has id1 and id2. id0 was evicted and written to disk.
        
        // Let's create a new pager directly to read from disk and verify id0 was written
        let mut direct_pager = Pager::open(&path)?;
        let read_page0 = direct_pager.read_page(id0)?;
        assert_eq!(read_page0.data[0], 100);

        Ok(())
    }
}
