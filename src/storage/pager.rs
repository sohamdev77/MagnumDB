//! The Pager is responsible for reading and writing fixed-size pages to disk.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const PAGE_SIZE: usize = 4096;
pub type PageId = u32;

/// A fixed size memory page.
#[derive(Clone)]
pub struct Page {
    pub data: [u8; PAGE_SIZE],
}

impl Default for Page {
    fn default() -> Self {
        Self {
            data: [0; PAGE_SIZE],
        }
    }
}

/// The Pager handles file I/O for pages.
pub struct Pager {
    file: File,
    pub num_pages: u32,
}

impl Pager {
    /// Opens or creates a database file.
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;

        let metadata = file.metadata()?;
        let file_size = metadata.len();
        let num_pages = (file_size / (PAGE_SIZE as u64)) as u32;

        let mut pager = Self { file, num_pages };

        if pager.num_pages == 0 {
            let mut meta_page = Page::default();
            // free_list_head = u32::MAX
            meta_page.data[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
            pager.write_page(0, &meta_page)?;
        }

        Ok(pager)
    }

    /// Reads a page from disk into memory.
    pub fn read_page(&mut self, page_id: PageId) -> io::Result<Page> {
        let mut page = Page::default();

        if page_id >= self.num_pages {
            return Ok(page);
        }

        let offset = (page_id as u64) * (PAGE_SIZE as u64);
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut page.data)?;

        Ok(page)
    }

    /// Writes a page to disk.
    pub fn write_page(&mut self, page_id: PageId, page: &Page) -> io::Result<()> {
        let offset = (page_id as u64) * (PAGE_SIZE as u64);
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(&page.data)?;

        if page_id >= self.num_pages {
            self.num_pages = page_id + 1;
        }

        Ok(())
    }

    /// Appends a new blank page or reuses a freed page from the free list.
    pub fn allocate_page(&mut self) -> io::Result<PageId> {
        let meta_page = self.read_page(0)?;
        let free_head = u32::from_le_bytes(meta_page.data[4..8].try_into().unwrap());

        if free_head != u32::MAX && free_head < self.num_pages && free_head != 0 {
            let free_page = self.read_page(free_head)?;
            let next_free = u32::from_le_bytes(free_page.data[0..4].try_into().unwrap());

            let mut updated_meta = meta_page;
            updated_meta.data[4..8].copy_from_slice(&next_free.to_le_bytes());
            self.write_page(0, &updated_meta)?;

            let blank_page = Page::default();
            self.write_page(free_head, &blank_page)?;

            Ok(free_head)
        } else {
            let page_id = self.num_pages;
            let blank_page = Page::default();
            self.write_page(page_id, &blank_page)?;
            Ok(page_id)
        }
    }

    /// Returns a page to the free list for future allocation.
    pub fn free_page(&mut self, page_id: PageId) -> io::Result<()> {
        if page_id == 0 || page_id >= self.num_pages {
            return Ok(());
        }

        let meta_page = self.read_page(0)?;
        let current_free_head = u32::from_le_bytes(meta_page.data[4..8].try_into().unwrap());

        let mut freed_page = Page::default();
        freed_page.data[0..4].copy_from_slice(&current_free_head.to_le_bytes());
        self.write_page(page_id, &freed_page)?;

        let mut updated_meta = meta_page;
        updated_meta.data[4..8].copy_from_slice(&page_id.to_le_bytes());
        self.write_page(0, &updated_meta)?;

        Ok(())
    }

    /// Syncs the underlying file to disk.
    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_pager_round_trip() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut pager = Pager::open(temp_file.path()).unwrap();

        // Page 0 initialized automatically
        assert_eq!(pager.num_pages, 1);

        let page_id = pager.allocate_page().unwrap();
        assert_eq!(page_id, 1);
        assert_eq!(pager.num_pages, 2);

        let mut page = Page::default();
        page.data[0] = 42;
        page.data[4095] = 99;

        pager.write_page(page_id, &page).unwrap();

        let read_page = pager.read_page(page_id).unwrap();
        assert_eq!(read_page.data[0], 42);
        assert_eq!(read_page.data[4095], 99);
    }

    #[test]
    fn test_pager_free_list_recycling() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut pager = Pager::open(temp_file.path()).unwrap();

        let page1 = pager.allocate_page().unwrap();
        let page2 = pager.allocate_page().unwrap();

        pager.free_page(page1).unwrap();

        let reused_page = pager.allocate_page().unwrap();
        assert_eq!(reused_page, page1);

        let new_page = pager.allocate_page().unwrap();
        assert_ne!(new_page, page1);
        assert_ne!(new_page, page2);
    }
}
