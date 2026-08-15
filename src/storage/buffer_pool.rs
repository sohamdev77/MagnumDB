use super::pager::{Page, PageId, Pager};
use lru::LruCache;
use parking_lot::{Mutex, RwLock};
use std::num::NonZeroUsize;
use std::sync::Arc;

pub type PageRef = Arc<RwLock<Page>>;

/// The Buffer Pool Manager is responsible for fetching pages from the Pager,
/// caching them in memory, and evicting the least recently used pages back to disk.
#[derive(Clone)]
pub struct BufferPool {
    pager: Arc<Mutex<Pager>>,
    /// Caches PageId -> (PageRef, is_dirty)
    cache: Arc<Mutex<LruCache<PageId, (PageRef, bool)>>>,
    /// Counter for auto-sync after N dirty writes
    dirty_write_count: Arc<Mutex<u32>>,
    /// Sync every N dirty writes. 0 = disabled.
    sync_interval: u32,
}

impl BufferPool {
    /// Creates a new BufferPool wrapping a Pager with a specific capacity limit.
    pub fn new(pager: Pager, capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity.max(1)).unwrap_or(NonZeroUsize::MIN);
        Self {
            pager: Arc::new(Mutex::new(pager)),
            cache: Arc::new(Mutex::new(LruCache::new(cap))),
            dirty_write_count: Arc::new(Mutex::new(0)),
            sync_interval: 0,
        }
    }

    /// Creates a new BufferPool with periodic auto-sync enabled.
    pub fn with_sync_interval(mut self, interval: u32) -> Self {
        self.sync_interval = interval;
        self
    }

    /// Fetches a page from the buffer pool or reads it from disk if not present.
    pub fn fetch_page(&self, page_id: PageId) -> anyhow::Result<PageRef> {
        {
            let mut cache = self.cache.lock();
            if let Some((page_ref, _)) = cache.get(&page_id) {
                return Ok(Arc::clone(page_ref));
            }
        }

        let page = {
            let mut pager = self.pager.lock();
            pager.read_page(page_id)?
        };

        let page_ref = Arc::new(RwLock::new(page));
        self.put_and_evict(page_id, Arc::clone(&page_ref), false)?;

        Ok(page_ref)
    }

    /// Writes a page to the buffer pool, marking it as dirty.
    pub fn write_page(&self, page_id: PageId, page: &Page) -> anyhow::Result<()> {
        let page_ref = {
            let mut cache = self.cache.lock();
            if let Some((page_ref, _)) = cache.get(&page_id) {
                Some(Arc::clone(page_ref))
            } else {
                None
            }
        };

        if let Some(page_ref) = page_ref {
            {
                let mut guard = page_ref.write();
                *guard = page.clone();
            }
            self.mark_dirty(page_id)?;
        } else {
            let page_ref = Arc::new(RwLock::new(page.clone()));
            self.put_and_evict(page_id, Arc::clone(&page_ref), true)?;
            
            if self.sync_interval > 0 {
                let mut dwc = self.dirty_write_count.lock();
                *dwc += 1;
                if *dwc >= self.sync_interval {
                    *dwc = 0;
                    self.flush_and_sync()?;
                }
            }
        }

        Ok(())
    }

    /// Marks a page as dirty in the buffer pool cache.
    pub fn mark_dirty(&self, page_id: PageId) -> anyhow::Result<()> {
        {
            let mut cache = self.cache.lock();
            if let Some((_, dirty)) = cache.get_mut(&page_id) {
                *dirty = true;
            }
        }

        if self.sync_interval > 0 {
            let mut dwc = self.dirty_write_count.lock();
            *dwc += 1;
            if *dwc >= self.sync_interval {
                *dwc = 0;
                self.flush_and_sync()?;
            }
        }

        Ok(())
    }

    /// Allocates a new page on disk and returns its ID.
    pub fn allocate_page(&self) -> anyhow::Result<PageId> {
        let mut pager = self.pager.lock();
        let page_id = pager.allocate_page()?;
        Ok(page_id)
    }

    /// Frees a page on disk and removes it from the cache.
    pub fn free_page(&self, page_id: PageId) -> anyhow::Result<()> {
        {
            let mut cache = self.cache.lock();
            cache.pop(&page_id);
        }
        let mut pager = self.pager.lock();
        pager.free_page(page_id)?;
        Ok(())
    }

    /// Helper function to put a page in the cache and evict the oldest if full.
    fn put_and_evict(&self, page_id: PageId, page_ref: PageRef, is_dirty: bool) -> anyhow::Result<()> {
        let mut cache = self.cache.lock();
        if cache.len() == cache.cap().get() && !cache.contains(&page_id) {
            if let Some((evict_id, (evict_ref, evict_dirty))) = cache.pop_lru() {
                if evict_dirty {
                    let mut pager = self.pager.lock();
                    let page_guard = evict_ref.read();
                    pager.write_page(evict_id, &*page_guard)?;
                }
            }
        }

        if let Some((_, curr_dirty)) = cache.get(&page_id) {
            let new_dirty = is_dirty || *curr_dirty;
            cache.put(page_id, (page_ref, new_dirty));
        } else {
            cache.put(page_id, (page_ref, is_dirty));
        }

        Ok(())
    }

    /// Flushes all dirty pages back to disk.
    pub fn flush_all(&self) -> anyhow::Result<()> {
        let mut to_flush = Vec::new();
        {
            let mut cache = self.cache.lock();
            for (page_id, (page_ref, dirty)) in cache.iter_mut() {
                if *dirty {
                    to_flush.push((*page_id, Arc::clone(page_ref)));
                    *dirty = false;
                }
            }
        }

        let mut pager = self.pager.lock();
        for (page_id, page_ref) in to_flush {
            let guard = page_ref.read();
            pager.write_page(page_id, &*guard)?;
        }

        Ok(())
    }

    /// Flushes all dirty pages and fsyncs the data file to disk.
    pub fn flush_and_sync(&self) -> anyhow::Result<()> {
        self.flush_all()?;
        let mut pager = self.pager.lock();
        pager.sync()?;
        Ok(())
    }

    /// Syncs the pager to disk.
    pub fn sync(&self) -> anyhow::Result<()> {
        let mut pager = self.pager.lock();
        pager.sync()?;
        Ok(())
    }

    /// Returns the total number of allocated pages on disk.
    pub fn get_num_pages(&self) -> u32 {
        let pager = self.pager.lock();
        pager.num_pages
    }

    /// Safely writes the checkpoint LSN to disk
    pub fn write_checkpoint_lsn(&self, lsn: u64) -> anyhow::Result<()> {
        let mut pager = self.pager.lock();
        pager.write_checkpoint_lsn(lsn)?;
        Ok(())
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
        let pool = BufferPool::new(pager, 2);

        let id0 = pool.allocate_page()?;
        let id1 = pool.allocate_page()?;
        let id2 = pool.allocate_page()?;

        {
            let p0 = pool.fetch_page(id0)?;
            p0.write().data[0] = 100;
            pool.mark_dirty(id0)?;
        }

        {
            let p1 = pool.fetch_page(id1)?;
            p1.write().data[0] = 101;
            pool.mark_dirty(id1)?;
        }

        {
            let p2 = pool.fetch_page(id2)?;
            p2.write().data[0] = 102;
            pool.mark_dirty(id2)?;
        }

        let mut direct_pager = Pager::open(&path)?;
        let read_page0 = direct_pager.read_page(id0)?;
        assert_eq!(read_page0.data[0], 100);

        Ok(())
    }

    #[test]
    fn test_buffer_pool_flush_and_sync() -> anyhow::Result<()> {
        let temp_file = NamedTempFile::new()?;
        let path = temp_file.path().to_path_buf();

        let pager = Pager::open(&path)?;
        let pool = BufferPool::new(pager, 10);

        let id = pool.allocate_page()?;
        {
            let p = pool.fetch_page(id)?;
            p.write().data[0] = 77;
            pool.mark_dirty(id)?;
        }

        pool.flush_and_sync()?;

        let mut direct_pager = Pager::open(&path)?;
        let read_page = direct_pager.read_page(id)?;
        assert_eq!(read_page.data[0], 77);

        Ok(())
    }
}
