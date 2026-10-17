//! Descriptor-bound macOS ACL access. No paths, shell commands or raw pointers
//! are exposed to callers. ACL text is parsed by the OS, never as a raw struct.
#![cfg(target_os = "macos")]
#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    ffi::{CString, c_char, c_int, c_void},
    fs::File,
    io,
    os::fd::AsRawFd,
    ptr::NonNull,
};

const MAX_TEXT: usize = 1024 * 1024;

// Signatures from the macOS SDK sys/acl.h. acl_t is an opaque pointer;
// ssize_t is pointer-sized and signed on supported macOS targets.
unsafe extern "C" {
    fn acl_get_fd(fd: c_int) -> *mut c_void;
    fn acl_init(count: c_int) -> *mut c_void;
    fn acl_set_fd(fd: c_int, acl: *mut c_void) -> c_int;
    fn acl_to_text(acl: *mut c_void, length: *mut isize) -> *mut c_char;
    fn acl_from_text(text: *const c_char) -> *mut c_void;
    fn acl_free(object: *mut c_void) -> c_int;
}

struct Allocation(NonNull<c_void>);
impl Allocation {
    fn new(pointer: *mut c_void) -> io::Result<Self> {
        NonNull::new(pointer)
            .map(Self)
            .ok_or_else(io::Error::last_os_error)
    }
}
impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: every Allocation owns exactly one non-null allocation returned
        // by an acl_* allocator; it is neither cloned nor freed elsewhere.
        unsafe {
            acl_free(self.0.as_ptr());
        }
    }
}

/// Capture text from the descriptor's inode, unaffected by pathname replacement.
pub fn capture(file: &File) -> io::Result<Vec<u8>> {
    // SAFETY: the borrowed File keeps its descriptor valid throughout the call.
    let acl = match Allocation::new(unsafe { acl_get_fd(file.as_raw_fd()) }) {
        Ok(acl) => acl,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // macOS reports ENOENT when the inode has no extended ACL.
            // SAFETY: zero is a valid entry count; returned allocation is owned.
            Allocation::new(unsafe { acl_init(0) })?
        }
        Err(error) => return Err(error),
    };
    encode(&acl)
}

/// Canonical representation of an empty extended ACL.
pub fn empty() -> io::Result<Vec<u8>> {
    // SAFETY: zero is a valid entry count; returned allocation is owned.
    let acl = Allocation::new(unsafe { acl_init(0) })?;
    encode(&acl)
}

fn encode(acl: &Allocation) -> io::Result<Vec<u8>> {
    let mut length = 0isize;
    // SAFETY: acl is an allocated ACL and length points to a live writable isize.
    let text = Allocation::new(unsafe { acl_to_text(acl.0.as_ptr(), &mut length) }.cast())?;
    let length = usize::try_from(length).map_err(|_| io::Error::other("negative ACL text size"))?;
    if length > MAX_TEXT {
        return Err(io::Error::other("ACL text exceeds budget"));
    }
    // SAFETY: acl_to_text reports the allocation's text length excluding its NUL;
    // the allocation remains live until after this bounded slice is copied.
    let bytes = unsafe { std::slice::from_raw_parts(text.0.as_ptr().cast::<u8>(), length) };
    Ok(bytes.to_vec())
}

/// Apply a saved ACL to a private candidate. Does not follow a pathname.
pub fn apply(file: &File, text: &[u8]) -> io::Result<()> {
    if text.len() > MAX_TEXT {
        return Err(io::Error::other("ACL text exceeds budget"));
    }
    let text = CString::new(text).map_err(|_| io::Error::other("NUL in ACL text"))?;
    // SAFETY: text is NUL terminated and remains live; OS validates text syntax.
    let acl = Allocation::new(unsafe { acl_from_text(text.as_ptr()) })?;
    // SAFETY: both descriptor and parsed ACL remain valid across the call.
    if unsafe { acl_set_fd(file.as_raw_fd(), acl.0.as_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn symlink_descriptor_acl_does_not_touch_target() {
        use std::os::unix::fs::{OpenOptionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        std::fs::write(&target, b"untouched").unwrap();
        let link = root.path().join("link");
        symlink("target", &link).unwrap();
        let entries = vec![exacl::AclEntry::allow_group(
            "everyone",
            exacl::Perm::READ,
            None,
        )];
        exacl::setfacl(&[&link], &entries, exacl::AclOption::SYMLINK_ACL).unwrap();
        let expected = exacl::getfacl(&link, exacl::AclOption::SYMLINK_ACL).unwrap();
        assert!(!expected.is_empty());
        let source = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_SYMLINK)
            .open(&link)
            .unwrap();
        assert!(source.metadata().unwrap().file_type().is_symlink());
        let saved = capture(&source).unwrap();
        std::fs::rename(&link, root.path().join("renamed")).unwrap();
        assert_eq!(capture(&source).unwrap(), saved);
        let candidate_path = root.path().join("candidate");
        symlink("target", &candidate_path).unwrap();
        let candidate = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_SYMLINK)
            .open(&candidate_path)
            .unwrap();
        apply(&candidate, &saved).unwrap();
        assert_eq!(
            exacl::getfacl(&candidate_path, exacl::AclOption::SYMLINK_ACL).unwrap(),
            expected
        );
        assert!(exacl::getfacl(&target, None).unwrap().is_empty());
        assert_eq!(std::fs::read(&target).unwrap(), b"untouched");
    }

    #[test]
    fn descriptor_acl_roundtrip_survives_rename_and_rejects_invalid_text() {
        let root = tempfile::tempdir().unwrap();
        let source_path = root.path().join("source");
        std::fs::write(&source_path, b"original").unwrap();
        let entries = vec![exacl::AclEntry::allow_group(
            "everyone",
            exacl::Perm::READ,
            None,
        )];
        exacl::setfacl(&[&source_path], &entries, None).unwrap();
        let expected = exacl::getfacl(&source_path, None).unwrap();
        assert!(!expected.is_empty());
        let source = File::open(&source_path).unwrap();
        let saved = capture(&source).unwrap();
        std::fs::rename(&source_path, root.path().join("renamed")).unwrap();
        std::fs::write(&source_path, b"different inode").unwrap();
        assert_eq!(capture(&source).unwrap(), saved);
        let candidate_path = root.path().join("candidate");
        let candidate = File::create(&candidate_path).unwrap();
        let empty = capture(&candidate).unwrap();
        apply(&candidate, &saved).unwrap();
        assert_eq!(exacl::getfacl(&candidate_path, None).unwrap(), expected);
        assert_eq!(capture(&candidate).unwrap(), saved);
        assert!(apply(&candidate, b"bad\0text").is_err());
        assert!(apply(&candidate, b"not an ACL").is_err());
        assert_eq!(capture(&candidate).unwrap(), saved);
        apply(&candidate, &empty).unwrap();
        assert!(exacl::getfacl(&candidate_path, None).unwrap().is_empty());
    }
}
