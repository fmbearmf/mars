use alloc::{string::ToString, sync::Arc};

use crate::sync::RwLock;
use file::File;
use inode::DirEntry;

pub mod file;
pub mod inode;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VfsError {
    NotFound,
    ExistsAlready,
    NotADirectory,
    IsADirectory,
    PermissionDenied,
    Io,
    OutOfSpace,
}

pub type Result<T> = core::result::Result<T, VfsError>;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FileType {
    Normal,
    Directory,
}

pub struct Vfs {
    pub root: Arc<RwLock<DirEntry>>,
}

impl Vfs {
    pub fn lookup(
        &self,
        path: &str,
        pwd: Option<Arc<RwLock<DirEntry>>>,
    ) -> Result<Arc<RwLock<DirEntry>>> {
        let mut current = if path.starts_with("/") || pwd.is_none() {
            self.root.clone()
        } else {
            pwd.unwrap()
        };

        for part in path.split("/") {
            if part.is_empty() || part == "." {
                continue;
            }

            if part == ".." {
                let parent_weak = current.read().parent.clone();

                if let Some(parent) = parent_weak.upgrade() {
                    current = parent;
                }

                continue;
            }

            current = self.lookup_child(&current, part)?;
        }

        Ok(current)
    }

    fn lookup_child(
        &self,
        parent: &Arc<RwLock<DirEntry>>,
        name: &str,
    ) -> Result<Arc<RwLock<DirEntry>>> {
        {
            let parent_re = parent.read();
            if parent_re.inode.file_type != FileType::Directory {
                return Err(VfsError::NotADirectory);
            }

            if let Some(child) = parent_re.children.read().get(name) {
                return Ok(child.clone());
            }
        }

        let parent_re = parent.read();
        let child_inode = parent_re.inode.operations.lookup_child(name)?;

        let mut children_wr = parent_re.children.write();

        if let Some(child) = children_wr.get(name) {
            return Ok(child.clone());
        }

        let new_dir_entry = Arc::new(RwLock::new(DirEntry::new(
            name.to_string(),
            child_inode,
            Arc::downgrade(parent),
        )));

        children_wr.insert(name.to_string(), new_dir_entry.clone());
        Ok(new_dir_entry)
    }

    pub fn open(&self, path: &str, create: bool, readable: bool, writable: bool) -> Result<File> {
        let dir_entry = match self.lookup(path, None) {
            Ok(dir) => dir,
            Err(VfsError::NotFound) if create => self.create_file(path)?,
            Err(e) => return Err(e),
        };

        let file_type = dir_entry.read().inode.file_type;
        if file_type == FileType::Directory && writable {
            return Err(VfsError::IsADirectory);
        }

        Ok(File::new(dir_entry, readable, writable))
    }

    fn create_file(&self, path: &str) -> Result<Arc<RwLock<DirEntry>>> {
        let (dir_path, name) = match path.rfind("/") {
            Some(i) => (&path[..i], &path[i + 1..]),
            None => ("/", path),
        };

        let parent_dir_entry = self.lookup(dir_path, None)?;
        let parent_re = parent_dir_entry.read();

        let new_inode = parent_re.inode.operations.create(name, FileType::Normal)?;

        let new_dir_entry = Arc::new(RwLock::new(DirEntry::new(
            name.to_string(),
            new_inode,
            Arc::downgrade(&parent_dir_entry),
        )));

        parent_re
            .children
            .write()
            .insert(name.to_string(), new_dir_entry.clone());
        Ok(new_dir_entry)
    }
}
