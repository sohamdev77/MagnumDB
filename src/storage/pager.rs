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

        Ok(Self { file, num_pages })
    }

    /// Reads a page from disk into memory.
    pub fn read_page(&mut self, page_id: PageId) -> io::Result<Page> {
        let mut page = Page::default();
        
        // If the page is out of bounds, we return a blank page for now.
        // In a strict implementation, reading out of bounds might be an error.
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

        // Update the number of pages if we appended
        if page_id >= self.num_pages {
            self.num_pages = page_id + 1;
        }

        Ok(())
    }

    /// Appends a new blank page to the file and returns its PageId.
    pub fn allocate_page(&mut self) -> io::Result<PageId> {
        let page_id = self.num_pages;
        let blank_page = Page::default();
        self.write_page(page_id, &blank_page)?;
        Ok(page_id)
    }

    /// Syncs the underlying file to disk.
    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }
}
