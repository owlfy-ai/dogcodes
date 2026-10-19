use super::{
    FileVersion, RecoveryRecord, ReviewResult, ReviewStore, marker, validate_relative, write_once,
};
use crate::review::FileKind;
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use std::io::{Read, Write};

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn supports_metadata(version: &FileVersion) -> bool {
    version.kind == FileKind::File
        || (cfg!(target_os = "macos") && version.kind == FileKind::Symlink)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn attribute_file(directory: &Dir, name: &str) -> ReviewResult<std::fs::File> {
    use rustix::fs::{Mode, OFlags, openat};
    #[cfg(target_os = "macos")]
    let link_flag = OFlags::from_bits_retain(libc::O_SYMLINK.unsigned_abs());
    #[cfg(not(target_os = "macos"))]
    let link_flag = OFlags::NOFOLLOW;
    let file: std::fs::File = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NONBLOCK | link_flag,
        Mode::empty(),
    )
    .map_err(|_| "cannot open attribute source without following links")?
    .into();
    let metadata = file
        .metadata()
        .map_err(|_| "attribute source metadata unavailable")?;
    if !(metadata.is_file() || (cfg!(target_os = "macos") && metadata.file_type().is_symlink())) {
        return Err("unsupported attribute source type".into());
    }
    Ok(file)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn capture_attributes(
    root: &str,
    path: &str,
) -> ReviewResult<super::xattrs::ExtendedAttributes> {
    let (directory, name) =
        parent(root, path, false)?.ok_or("attribute source parent unavailable")?;
    super::xattrs::ExtendedAttributes::capture_stable(&attribute_file(&directory, &name)?)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn saved_attributes(
    store: &ReviewStore,
    record: &RecoveryRecord,
) -> ReviewResult<super::xattrs::ExtendedAttributes> {
    match &record.original_xattrs {
        Some(hash) => serde_json::from_slice(&store.read_blob(hash)?)
            .map_err(|_| "invalid saved extended attributes".into()),
        None if record
            .original
            .as_ref()
            .is_none_or(|v| !supports_metadata(v)) =>
        {
            Ok(super::xattrs::ExtendedAttributes::default())
        }
        None => Err("missing original extended attributes".into()),
    }
}

#[cfg(target_os = "macos")]
fn stable_acl(file: &std::fs::File) -> ReviewResult<Vec<u8>> {
    let acl = native_acl::capture(file).map_err(|_| "cannot read file ACL")?;
    if acl != native_acl::capture(file).map_err(|_| "cannot reread file ACL")? {
        return Err("file ACL changed during capture".into());
    }
    Ok(acl)
}

#[cfg(target_os = "macos")]
pub(super) fn capture_acl(root: &str, path: &str) -> ReviewResult<Vec<u8>> {
    let (directory, name) = parent(root, path, false)?.ok_or("ACL source parent unavailable")?;
    stable_acl(&attribute_file(&directory, &name)?)
}

#[cfg(target_os = "macos")]
fn saved_acl(store: &ReviewStore, record: &RecoveryRecord) -> ReviewResult<Vec<u8>> {
    match &record.original_acl {
        Some(hash) => store.read_blob(hash),
        None if record
            .original
            .as_ref()
            .is_none_or(|v| !supports_metadata(v)) =>
        {
            native_acl::empty().map_err(|_| "cannot initialize empty ACL".into())
        }
        None => Err("missing original ACL backup".into()),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn file_owner(file: &std::fs::File) -> ReviewResult<super::FileOwner> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().map_err(|_| "file ownership unavailable")?;
    Ok(super::FileOwner {
        uid: metadata.uid(),
        gid: metadata.gid(),
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn capture_owner(root: &str, path: &str) -> ReviewResult<super::FileOwner> {
    let (directory, name) =
        parent(root, path, false)?.ok_or("ownership source parent unavailable")?;
    file_owner(&attribute_file(&directory, &name)?)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn saved_owner(record: &RecoveryRecord) -> ReviewResult<Option<&super::FileOwner>> {
    if record.original.as_ref().is_some_and(supports_metadata) && record.original_owner.is_none() {
        return Err("missing original file ownership".into());
    }
    Ok(record.original_owner.as_ref())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn apply_owner(record: &RecoveryRecord, file: &std::fs::File) -> ReviewResult<()> {
    use rustix::fs::{Gid, Uid, fchown};
    if let Some(owner) = saved_owner(record)? {
        if file_owner(file)? != *owner {
            fchown(
                file,
                Some(Uid::from_raw(owner.uid)),
                Some(Gid::from_raw(owner.gid)),
            )
            .map_err(|_| "cannot preserve file ownership; original retained")?;
        }
        if file_owner(file)? != *owner {
            return Err("replacement file ownership verification failed".into());
        }
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn verify_attributes(
    store: &ReviewStore,
    record: &RecoveryRecord,
    directory: &Dir,
    name: &str,
    version: Option<&FileVersion>,
) -> ReviewResult<()> {
    #[cfg(target_os = "macos")]
    if version.is_some_and(supports_metadata)
        && stable_acl(&attribute_file(directory, name)?)? != saved_acl(store, record)?
    {
        return Err("file ACL changed; recovery refused and backup retained".into());
    }
    if version.is_some_and(supports_metadata)
        && let Some(owner) = saved_owner(record)?
        && file_owner(&attribute_file(directory, name)?)? != *owner
    {
        return Err("file ownership changed; recovery refused and backup retained".into());
    }
    if version.is_some_and(supports_metadata)
        && super::xattrs::ExtendedAttributes::capture_stable(&attribute_file(directory, name)?)?
            != saved_attributes(store, record)?
    {
        return Err("extended attributes changed; recovery refused and backup retained".into());
    }
    Ok(())
}

// Directory sync only exists on Unix; the Result serves that path.
#[cfg_attr(not(unix), allow(clippy::unnecessary_wraps))]
pub(super) fn sync_directory(directory: &Dir) -> ReviewResult<()> {
    // On Linux cap-std may hold directories as O_PATH descriptors, which cannot
    // be fsynced (EBADF). Open the same directory read-only just for the sync.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use rustix::fs::{Mode, OFlags};
        let readable = rustix::fs::openat(
            directory,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| "cannot open transaction directory for sync")?;
        rustix::fs::fsync(&readable).map_err(|_| "cannot sync transaction directory")?;
    }
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
    directory
        .try_clone()
        .map_err(|_| "cannot clone directory for sync")?
        .into_std_file()
        .sync_all()
        .map_err(|_| "cannot sync transaction directory")?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

pub(super) fn parent(root: &str, path: &str, create: bool) -> ReviewResult<Option<(Dir, String)>> {
    validate_relative(path)?;
    let mut directory = Dir::open_ambient_dir(root, ambient_authority())
        .map_err(|_| "workspace root unavailable")?;
    let parts: Vec<_> = path.split('/').collect();
    for component in &parts[..parts.len() - 1] {
        let metadata = match directory.symlink_metadata(component) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                match directory.create_dir(component) {
                    Ok(()) => {
                        sync_directory(&directory)?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(_) => return Err("cannot recreate missing parent directory".into()),
                }
                directory
                    .symlink_metadata(component)
                    .map_err(|_| "created parent unavailable")?
            }
            Err(_) => return Err("parent directory unavailable".into()),
        };
        if !metadata.is_dir() {
            return Err("parent must be a real directory, not a symlink".into());
        }
        directory = directory
            .open_dir(component)
            .map_err(|_| "cannot open parent directory")?;
    }
    Ok(Some((
        directory,
        parts.last().ok_or("missing filename")?.to_string(),
    )))
}

fn version(store: &ReviewStore, directory: &Dir, name: &str) -> ReviewResult<Option<FileVersion>> {
    let metadata = match directory.symlink_metadata(name) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("cannot inspect current file".into()),
    };
    let symlink = metadata.file_type().is_symlink();
    if !metadata.is_file() && !symlink {
        return Err("not a regular file or symlink".into());
    }
    let bytes = if symlink {
        directory
            .read_link_contents(name)
            .map_err(|_| "cannot read symlink")?
            .to_str()
            .ok_or("non UTF-8 symlink")?
            .as_bytes()
            .to_vec()
    } else {
        if metadata.len() > 4 * 1024 * 1024 {
            return Err("file exceeds restore size budget".into());
        }
        let mut bytes = Vec::new();
        directory
            .open(name)
            .map_err(|_| "cannot open current file")?
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read current file")?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("file grew beyond restore budget".into());
        }
        bytes
    };
    #[cfg(unix)]
    let executable = {
        use cap_std::fs::PermissionsExt;
        !symlink && metadata.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = false;
    Ok(Some(FileVersion {
        hash: store.put_blob(&bytes)?,
        bytes: bytes.len() as u64,
        kind: if symlink {
            FileKind::Symlink
        } else {
            FileKind::File
        },
        executable,
        binary: bytes.contains(&0),
    }))
}

pub(super) fn current_version(
    store: &ReviewStore,
    root: &str,
    path: &str,
) -> ReviewResult<Option<FileVersion>> {
    match parent(root, path, false)? {
        Some((directory, name)) => version(store, &directory, &name),
        None => Ok(None),
    }
}

pub(super) fn file_mode(root: &str, path: &str) -> ReviewResult<Option<u32>> {
    let Some((directory, name)) = parent(root, path, false)? else {
        return Ok(None);
    };
    let metadata = match directory.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("cannot inspect original permissions".into()),
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt;
        Ok(Some(metadata.permissions().mode() & 0o7777))
    }
    #[cfg(not(unix))]
    {
        Ok(Some(u32::from(metadata.permissions().readonly())))
    }
}

fn publish(directory: &Dir, temporary: &Dir, name: &str, source: &str) -> ReviewResult<()> {
    // link is exclusive: if another writer creates name, do not overwrite it.
    temporary
        .hard_link(source, directory, name)
        .map_err(|_| "destination was recreated or cannot be linked; backup retained".into())
}

#[cfg(unix)]
fn replacement_mode(record: &RecoveryRecord, desired: &FileVersion) -> u32 {
    let original = record.original_mode.unwrap_or(0o644);
    if record
        .original
        .as_ref()
        .is_some_and(|v| v.executable == desired.executable)
    {
        original
    } else {
        (original & !0o111) | if desired.executable { 0o111 } else { 0 }
    }
}

#[cfg(unix)]
fn verify_recovery_mode(
    record: &RecoveryRecord,
    directory: &Dir,
    name: &str,
    version: Option<&FileVersion>,
    original: bool,
) -> ReviewResult<()> {
    use cap_std::fs::PermissionsExt;
    let Some(version) = version.filter(|v| v.kind == FileKind::File) else {
        return Ok(());
    };
    let expected = if original {
        record
            .original_mode
            .ok_or("missing original file permissions")?
    } else {
        replacement_mode(record, version)
    };
    let metadata = directory
        .symlink_metadata(name)
        .map_err(|_| "recovery permissions unavailable")?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o7777 != expected {
        return Err("file permissions changed; recovery refused and backup retained".into());
    }
    Ok(())
}

fn replace(
    store: &ReviewStore,
    record: &RecoveryRecord,
    expected: Option<&FileVersion>,
    desired: Option<&FileVersion>,
    phase: &str,
) -> ReviewResult<()> {
    replace_with_pre_capture(store, record, expected, desired, phase, || {})
}

fn replace_with_pre_capture(
    store: &ReviewStore,
    record: &RecoveryRecord,
    expected: Option<&FileVersion>,
    desired: Option<&FileVersion>,
    phase: &str,
    pre_capture: impl FnOnce(),
) -> ReviewResult<()> {
    if expected
        .into_iter()
        .chain(desired)
        .any(|v| v.kind == FileKind::Gitlink)
    {
        return Err("submodule pointers cannot be restored as ordinary files".into());
    }
    let (directory, name) = parent(
        &record.root,
        &record.path,
        expected.is_none() && desired.is_some(),
    )?
    .ok_or("parent directory was removed since preview")?;
    if version(store, &directory, &name)?.as_ref() != expected {
        return Err("file changed since preview; refusing replacement".into());
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    verify_attributes(store, record, &directory, &name, expected)?;
    if phase == "apply"
        && expected.is_some()
        && file_mode(&record.root, &record.path)? != record.original_mode
    {
        return Err("file permissions changed since confirmation".into());
    }
    #[cfg(unix)]
    if phase == "undo"
        && let Some(expected) = expected
        && expected.kind == FileKind::File
        && file_mode(&record.root, &record.path)? != Some(replacement_mode(record, expected))
    {
        return Err("file permissions changed after operation; refusing undo".into());
    }
    let transaction_name = format!(".agent-review-{}-{phase}", record.id);
    directory
        .create_dir(&transaction_name)
        .map_err(|_| "transaction directory exists or cannot be created; recovery required")?;
    let temporary = directory
        .open_dir(&transaction_name)
        .map_err(|_| "cannot open transaction directory")?;
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt;
        directory
            .set_permissions(
                &transaction_name,
                cap_std::fs::Permissions::from_mode(0o700),
            )
            .map_err(|_| "cannot protect transaction backup")?;
    }
    if let Some(desired) = desired {
        let bytes = store.read_blob(&desired.hash)?;
        if desired.kind == FileKind::Symlink {
            let target = std::str::from_utf8(&bytes).map_err(|_| "non UTF-8 symlink target")?;
            #[cfg(not(windows))]
            temporary
                .symlink_contents(target, "replacement")
                .map_err(|_| "cannot prepare symlink")?;
            #[cfg(windows)]
            temporary
                .symlink_file(target, "replacement")
                .map_err(|_| "cannot prepare symlink; check Windows privileges")?;
            #[cfg(target_os = "macos")]
            {
                let candidate = attribute_file(&temporary, "replacement")?;
                apply_owner(record, &candidate)?;
                saved_attributes(store, record)?.apply_to_candidate(&candidate)?;
                let acl = saved_acl(store, record)?;
                native_acl::apply(&candidate, &acl).map_err(|_| "cannot preserve symlink ACL")?;
                if stable_acl(&candidate)? != acl {
                    return Err("symlink ACL verification failed".into());
                }
            }
        } else {
            let mut file = temporary
                .open_with(
                    "replacement",
                    OpenOptions::new().write(true).create_new(true),
                )
                .map_err(|_| "cannot prepare replacement")?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| "cannot persist replacement")?;
            // chown may clear special permission bits; do it before chmod/xattrs.
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            apply_owner(
                record,
                &file
                    .try_clone()
                    .map_err(|_| "cannot clone replacement")?
                    .into_std(),
            )?;
            #[cfg(unix)]
            {
                use cap_std::fs::PermissionsExt;
                temporary
                    .set_permissions(
                        "replacement",
                        cap_std::fs::Permissions::from_mode(if phase == "undo" {
                            record.original_mode.unwrap_or(if desired.executable {
                                0o755
                            } else {
                                0o644
                            })
                        } else {
                            replacement_mode(record, desired)
                        }),
                    )
                    .map_err(|_| "cannot set replacement mode")?;
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            saved_attributes(store, record)?.apply_to_candidate(
                &file
                    .try_clone()
                    .map_err(|_| "cannot clone candidate")?
                    .into_std(),
            )?;
            #[cfg(target_os = "macos")]
            {
                let candidate = file
                    .try_clone()
                    .map_err(|_| "cannot clone ACL candidate")?
                    .into_std();
                let acl = saved_acl(store, record)?;
                native_acl::apply(&candidate, &acl).map_err(|_| "cannot preserve file ACL")?;
                if stable_acl(&candidate)? != acl {
                    return Err("candidate ACL verification failed".into());
                }
            }
            file.sync_all()
                .map_err(|_| "cannot persist replacement metadata")?;
        }
    }
    sync_directory(&temporary)?;
    sync_directory(&directory)?;
    pre_capture();
    if expected.is_some() {
        directory
            .rename(&name, &temporary, "captured")
            .map_err(|_| "cannot capture original; replacement aborted")?;
        sync_directory(&temporary)?;
        sync_directory(&directory)?;
        if version(store, &temporary, "captured")?.as_ref() != expected {
            let _ = publish(&directory, &temporary, &name, "captured");
            return Err(format!(
                "file changed while being captured; backup retained in {transaction_name}"
            ));
        }
        #[cfg(unix)]
        if let Err(error) =
            verify_recovery_mode(record, &temporary, "captured", expected, phase == "apply")
        {
            // Content hashes only encode executability, not all permission
            // bits. Preserve chmod changes made during candidate preparation.
            let _ = publish(&directory, &temporary, &name, "captured");
            sync_directory(&directory)?;
            return Err(format!(
                "{error}; captured backup retained in {transaction_name}"
            ));
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if let Err(error) = verify_attributes(store, record, &temporary, "captured", expected) {
            let _ = publish(&directory, &temporary, &name, "captured");
            sync_directory(&directory)?;
            return Err(error);
        }
    }
    if desired.is_some()
        && let Err(error) = publish(&directory, &temporary, &name, "replacement")
    {
        if expected.is_some() {
            let _ = publish(&directory, &temporary, &name, "captured");
        }
        return Err(format!("{error}: {transaction_name}"));
    }
    if version(store, &directory, &name)?.as_ref() != desired {
        return Err(format!(
            "file changed during replacement; backup retained in {transaction_name}"
        ));
    }
    sync_directory(&directory)?;
    // Durable original bytes are in CAS. Remove only our named temporary links
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    verify_attributes(store, record, &directory, &name, desired)?;
    // after success, never recursively delete a directory with unknown entries.
    if expected.is_some() {
        temporary
            .remove_file("captured")
            .map_err(|_| "cannot clean captured link; recovery required")?;
    }
    if desired.is_some() {
        temporary
            .remove_file("replacement")
            .map_err(|_| "cannot clean replacement link; recovery required")?;
    }
    // Windows cannot remove a directory while its capability handle is open.
    drop(temporary);
    directory
        .remove_dir(&transaction_name)
        .map_err(|_| "cannot clean transaction directory; recovery required")?;
    sync_directory(&directory).map_err(|_| "cannot sync parent")?;
    Ok(())
}

impl ReviewStore {
    /// Explicit recovery of interrupted apply/undo. Never infer ownership of
    /// changed bytes: only logged versions and validated temporary files qualify.
    pub fn reconcile_recovery(&self, task_id: &str, id: &str) -> ReviewResult<()> {
        let entry = self.recovery_record(task_id, id)?;
        if entry.undone {
            return Ok(());
        }
        let undo = marker(
            &self
                .directory
                .join("recovery")
                .join(format!("{id}.undo-started")),
            b"started",
        )?;
        if entry.applied && !undo {
            return Ok(());
        }
        let phase = if undo { "undo" } else { "apply" };
        let (expected, desired) = if undo {
            (&entry.record.replacement, &entry.record.original)
        } else {
            (&entry.record.original, &entry.record.replacement)
        };
        let location = parent(&entry.record.root, &entry.record.path, false)?;
        let current = match &location {
            Some((directory, name)) => version(self, directory, name)?,
            None => None,
        };
        if current != *expected && current != *desired && current.is_some() {
            return Err("file contains newer work; recovery refused".into());
        }
        if let Some((directory, name)) = &location {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            verify_attributes(self, &entry.record, directory, name, current.as_ref())?;
            #[cfg(unix)]
            verify_recovery_mode(
                &entry.record,
                directory,
                name,
                current.as_ref(),
                if current == *expected { !undo } else { undo },
            )?;
            let transaction_name = format!(".agent-review-{id}-{phase}");
            let temporary = match directory.symlink_metadata(&transaction_name) {
                Ok(metadata) if metadata.is_dir() => Some(
                    directory
                        .open_dir(&transaction_name)
                        .map_err(|_| "cannot open interrupted transaction")?,
                ),
                Ok(_) => return Err("invalid transaction directory".into()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return Err("transaction directory unavailable".into()),
            };
            if let Some(temporary) = temporary {
                let captured = version(self, &temporary, "captured")?;
                let replacement = version(self, &temporary, "replacement")?;
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                {
                    verify_attributes(
                        self,
                        &entry.record,
                        &temporary,
                        "captured",
                        captured.as_ref(),
                    )?;
                    verify_attributes(
                        self,
                        &entry.record,
                        &temporary,
                        "replacement",
                        replacement.as_ref(),
                    )?;
                }
                if captured.is_some() && captured != *expected
                    || replacement.is_some() && replacement != *desired
                {
                    return Err("transaction backup changed; manual recovery required".into());
                }
                #[cfg(unix)]
                {
                    verify_recovery_mode(
                        &entry.record,
                        &temporary,
                        "captured",
                        captured.as_ref(),
                        !undo,
                    )?;
                    verify_recovery_mode(
                        &entry.record,
                        &temporary,
                        "replacement",
                        replacement.as_ref(),
                        undo,
                    )?;
                }
                if current.is_none() && desired.is_some() && expected.is_some() {
                    if captured != *expected {
                        return Err("interrupted transaction has no verified original".into());
                    }
                    publish(directory, &temporary, name, "captured")?;
                }
                if captured.is_some() {
                    temporary
                        .remove_file("captured")
                        .map_err(|_| "cannot clean verified backup link")?;
                }
                if replacement.is_some() {
                    temporary
                        .remove_file("replacement")
                        .map_err(|_| "cannot clean verified replacement link")?;
                }
                drop(temporary);
                directory.remove_dir(&transaction_name).map_err(
                    |_| "unknown transaction entries retained; manual recovery required",
                )?;
                sync_directory(directory)?;
            } else if current.is_none() && expected.is_some() && desired.is_some() {
                return Err(
                    "missing target and transaction backup; manual recovery required".into(),
                );
            }
        }
        if current == *desired {
            if undo {
                self.mark_recovery_undone(task_id, id)
            } else {
                self.mark_recovery_applied(task_id, id)
            }
        } else if undo {
            self.undo_recovery(task_id, id)
        } else {
            self.apply_recovery(task_id, id)
        }
    }

    pub fn apply_recovery(&self, task_id: &str, id: &str) -> ReviewResult<()> {
        let entry = self.recovery_record(task_id, id)?;
        if entry.applied || entry.undone {
            return Err("operation was already applied".into());
        }
        replace(
            self,
            &entry.record,
            entry.record.original.as_ref(),
            entry.record.replacement.as_ref(),
            "apply",
        )?;
        self.mark_recovery_applied(task_id, id)
    }

    pub fn undo_recovery(&self, task_id: &str, id: &str) -> ReviewResult<()> {
        let entry = self.recovery_record(task_id, id)?;
        let current = match parent(&entry.record.root, &entry.record.path, false)? {
            Some((directory, name)) => version(self, &directory, &name)?,
            None => None,
        };
        let record = self.check_recovery_undo(task_id, id, &current)?;
        write_once(
            &self
                .directory
                .join("recovery")
                .join(format!("{id}.undo-started")),
            b"started",
        )?;
        replace(
            self,
            &record,
            record.replacement.as_ref(),
            record.original.as_ref(),
            "undo",
        )?;
        self.mark_recovery_undone(task_id, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(target_os = "macos")]
    #[test]
    fn symlink_metadata_roundtrip_and_target_isolation() {
        use rustix::fs::{XattrFlags, fsetxattr};
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        fs::write(root.path().join("target"), b"keep").unwrap();
        fs::write(root.path().join("other"), b"also keep").unwrap();
        let target_attrs = capture_attributes(root.path().to_str().unwrap(), "target").unwrap();
        let target_acl = capture_acl(root.path().to_str().unwrap(), "target").unwrap();
        let target_owner = capture_owner(root.path().to_str().unwrap(), "target").unwrap();
        directory.symlink_contents("target", "link").unwrap();
        let path = root.path().join("link");
        let entries = vec![exacl::AclEntry::allow_group(
            "everyone",
            exacl::Perm::READ,
            None,
        )];
        exacl::setfacl(&[&path], &entries, exacl::AclOption::SYMLINK_ACL).unwrap();
        let set = |value: &[u8]| {
            fsetxattr(
                attribute_file(&directory, "link").unwrap(),
                "user.dogcodes.link",
                value,
                XattrFlags::empty(),
            )
            .unwrap();
        };
        set(&[0, 255, 3]);
        let attrs = capture_attributes(root.path().to_str().unwrap(), "link").unwrap();
        let acl = capture_acl(root.path().to_str().unwrap(), "link").unwrap();
        let owner = capture_owner(root.path().to_str().unwrap(), "link").unwrap();
        let original = version(&store, &directory, "link").unwrap();
        let record = store
            .prepare_recovery(
                "task",
                root.path(),
                "link",
                original,
                Some(FileVersion {
                    hash: store.put_blob(b"other").unwrap(),
                    bytes: 5,
                    kind: FileKind::Symlink,
                    executable: false,
                    binary: false,
                }),
            )
            .unwrap();
        set(b"new attributes");
        assert!(store.apply_recovery("task", &record.id).is_err());
        assert_eq!(
            fs::read_link(&path).unwrap(),
            std::path::Path::new("target")
        );
        set(&[0, 255, 3]);
        store.apply_recovery("task", &record.id).unwrap();
        assert_eq!(fs::read_link(&path).unwrap(), std::path::Path::new("other"));
        assert_eq!(
            capture_attributes(root.path().to_str().unwrap(), "link").unwrap(),
            attrs
        );
        assert_eq!(
            capture_acl(root.path().to_str().unwrap(), "link").unwrap(),
            acl
        );
        assert_eq!(
            capture_owner(root.path().to_str().unwrap(), "link").unwrap(),
            owner
        );
        drop(store);
        let store = ReviewStore::open(data.path().into()).unwrap();
        store.undo_recovery("task", &record.id).unwrap();
        assert_eq!(
            fs::read_link(&path).unwrap(),
            std::path::Path::new("target")
        );
        assert_eq!(
            capture_attributes(root.path().to_str().unwrap(), "link").unwrap(),
            attrs
        );
        assert_eq!(
            capture_acl(root.path().to_str().unwrap(), "link").unwrap(),
            acl
        );
        assert_eq!(
            capture_owner(root.path().to_str().unwrap(), "link").unwrap(),
            owner
        );
        assert_eq!(fs::read(root.path().join("target")).unwrap(), b"keep");
        assert_eq!(fs::read(root.path().join("other")).unwrap(), b"also keep");
        assert_eq!(
            capture_attributes(root.path().to_str().unwrap(), "target").unwrap(),
            target_attrs
        );
        assert_eq!(
            capture_acl(root.path().to_str().unwrap(), "target").unwrap(),
            target_acl
        );
        assert_eq!(
            capture_owner(root.path().to_str().unwrap(), "target").unwrap(),
            target_owner
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn acl_apply_reopen_undo_drift_and_cleanup_protection() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        let path = root.path().join("a");
        fs::write(&path, b"work").unwrap();
        let entries = vec![exacl::AclEntry::allow_group(
            "everyone",
            exacl::Perm::READ,
            None,
        )];
        let set = || exacl::setfacl(&[&path], &entries, None).unwrap();
        let clear = || exacl::setfacl(&[&path], &[], None).unwrap();
        set();
        let expected_acl = exacl::getfacl(&path, None).unwrap();
        let original = version(&store, &directory, "a").unwrap();
        let record = store
            .prepare_recovery(
                "task",
                root.path(),
                "a",
                original,
                Some(FileVersion {
                    hash: store.put_blob(b"base").unwrap(),
                    bytes: 4,
                    kind: FileKind::File,
                    executable: false,
                    binary: false,
                }),
            )
            .unwrap();
        let hash = record.original_acl.clone().unwrap();
        clear();
        assert!(
            store
                .apply_recovery("task", &record.id)
                .unwrap_err()
                .contains("ACL changed")
        );
        assert_eq!(fs::read(&path).unwrap(), b"work");
        set();
        store.apply_recovery("task", &record.id).unwrap();
        assert_eq!(exacl::getfacl(&path, None).unwrap(), expected_acl);
        drop(store);
        let store = ReviewStore::open(data.path().into()).unwrap();
        clear();
        assert!(
            store
                .undo_recovery("task", &record.id)
                .unwrap_err()
                .contains("ACL changed")
        );
        assert_eq!(fs::read(&path).unwrap(), b"base");
        set();
        store.undo_recovery("task", &record.id).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"work");
        assert_eq!(exacl::getfacl(&path, None).unwrap(), expected_acl);
        let bytes = store.read_blob(&hash).unwrap();
        drop(store);
        fs::File::open(data.path().join("blobs").join(&hash))
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_hours(24 * 8))
            .unwrap();
        let store = ReviewStore::try_open_maintenance(data.path().into()).unwrap();
        let preview = store.preview_storage_cleanup().unwrap();
        store.execute_storage_cleanup(&preview).unwrap();
        assert_eq!(store.read_blob(&hash).unwrap(), bytes);
        assert_eq!(store.storage_usage([]).unwrap().missing_references, 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "macOS devfs ACL behavior; this alias is NOT a valid ACL source"]
    fn macos_acl_descriptor_alias_does_not_preserve_acl() {
        use std::os::fd::AsRawFd;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source");
        fs::write(&path, b"original").unwrap();
        let entries = vec![exacl::AclEntry::allow_group(
            "everyone",
            exacl::Perm::READ,
            None,
        )];
        exacl::setfacl(&[&path], &entries, None).unwrap();
        let expected = exacl::getfacl(&path, None).unwrap();
        assert!(!expected.is_empty());
        let source = fs::File::open(&path).unwrap();
        fs::rename(&path, root.path().join("renamed")).unwrap();
        fs::write(&path, b"new unrelated inode").unwrap();
        let descriptor_path = format!("/dev/fd/{}", source.as_raw_fd());
        let actual = exacl::getfacl(&descriptor_path, None).unwrap();
        assert_eq!(
            exacl::getfacl(root.path().join("renamed"), None).unwrap(),
            expected
        );
        assert!(actual.is_empty(), "reassess devfs behavior if this changes");
        assert_ne!(actual, expected, "do not use devfs aliases to back up ACLs");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    #[ignore = "requires permission to assign a supplementary group different from the fixture directory"]
    fn actual_group_preservation_and_capture_drift_both_directions() {
        use rustix::fs::{Gid, fchown};
        for undo in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let data = tempfile::tempdir().unwrap();
            let store = ReviewStore::open(data.path().into()).unwrap();
            let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
            let path = root.path().join("a");
            fs::write(&path, b"work").unwrap();
            let initial = file_owner(&fs::File::open(&path).unwrap()).unwrap();
            let other = rustix::process::getgroups()
                .unwrap()
                .into_iter()
                .find(|group| group.as_raw() != initial.gid)
                .expect("acceptance requires another supplementary group");
            let set_group = |group| {
                fchown(fs::File::open(&path).unwrap(), None, Some(group)).unwrap();
            };
            set_group(other);
            let original = version(&store, &directory, "a").unwrap();
            let record = store
                .prepare_recovery(
                    "task",
                    root.path(),
                    "a",
                    original,
                    Some(FileVersion {
                        hash: store.put_blob(b"base").unwrap(),
                        bytes: 4,
                        kind: FileKind::File,
                        executable: false,
                        binary: false,
                    }),
                )
                .unwrap();
            assert_eq!(record.original_owner.as_ref().unwrap().gid, other.as_raw());
            // Newly created private candidates inherit the directory's original group,
            // so forward and undo must actually execute fchown, not just its no-op path.
            store.apply_recovery("task", &record.id).unwrap();
            assert_eq!(
                file_owner(&fs::File::open(&path).unwrap()).unwrap().gid,
                other.as_raw()
            );
            drop(store);
            let store = ReviewStore::open(data.path().into()).unwrap();
            store.undo_recovery("task", &record.id).unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"work");
            assert_eq!(
                file_owner(&fs::File::open(&path).unwrap()).unwrap().gid,
                other.as_raw()
            );

            let record = store
                .prepare_recovery(
                    "task",
                    root.path(),
                    "a",
                    record.original,
                    record.replacement,
                )
                .unwrap();
            if undo {
                store.apply_recovery("task", &record.id).unwrap();
            }
            let (expected, desired, phase) = if undo {
                (
                    record.replacement.as_ref(),
                    record.original.as_ref(),
                    "undo",
                )
            } else {
                (
                    record.original.as_ref(),
                    record.replacement.as_ref(),
                    "apply",
                )
            };
            let error = replace_with_pre_capture(&store, &record, expected, desired, phase, || {
                set_group(Gid::from_raw(initial.gid));
            })
            .unwrap_err();
            assert!(error.contains("ownership changed"), "{error}");
            assert_eq!(
                fs::read(&path).unwrap(),
                if undo { b"base" } else { b"work" }
            );
            assert_eq!(
                file_owner(&fs::File::open(&path).unwrap()).unwrap(),
                initial
            );
            let captured = root
                .path()
                .join(format!(".agent-review-{}-{phase}/captured", record.id));
            assert_eq!(
                file_owner(&fs::File::open(captured).unwrap()).unwrap(),
                initial
            );
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn ownership_survives_reopen_and_rejects_mismatched_or_missing_metadata() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        let path = root.path().join("a");
        fs::write(&path, b"work").unwrap();
        let owner = capture_owner(root.path().to_str().unwrap(), "a").unwrap();
        let original = version(&store, &directory, "a").unwrap();
        let record = store
            .prepare_recovery(
                "task",
                root.path(),
                "a",
                original.clone(),
                Some(FileVersion {
                    hash: store.put_blob(b"base").unwrap(),
                    bytes: 4,
                    kind: FileKind::File,
                    executable: false,
                    binary: false,
                }),
            )
            .unwrap();
        assert_eq!(record.original_owner.as_ref(), Some(&owner));
        let mut invalid = record.clone();
        invalid.original_owner.as_mut().unwrap().gid ^= 1;
        assert!(verify_attributes(&store, &invalid, &directory, "a", original.as_ref()).is_err());
        invalid.original_owner = None;
        assert!(verify_attributes(&store, &invalid, &directory, "a", original.as_ref()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"work");
        store.apply_recovery("task", &record.id).unwrap();
        assert_eq!(
            capture_owner(root.path().to_str().unwrap(), "a").unwrap(),
            owner
        );
        drop(store);
        let store = ReviewStore::open(data.path().into()).unwrap();
        store.undo_recovery("task", &record.id).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"work");
        assert_eq!(
            capture_owner(root.path().to_str().unwrap(), "a").unwrap(),
            owner
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn extended_attributes_survive_apply_reopen_undo_and_cache_cleanup() {
        use rustix::fs::{XattrFlags, fsetxattr};
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        let path = root.path().join("a");
        fs::write(&path, b"work").unwrap();
        let set = |value: &[u8]| {
            fsetxattr(
                fs::File::open(&path).unwrap(),
                "user.dogcodes.test",
                value,
                XattrFlags::empty(),
            )
            .unwrap();
        };
        set(&[0, 255, 3]);
        let attributes = capture_attributes(root.path().to_str().unwrap(), "a").unwrap();
        let original = version(&store, &directory, "a").unwrap();
        let replacement = Some(FileVersion {
            hash: store.put_blob(b"base").unwrap(),
            bytes: 4,
            kind: FileKind::File,
            executable: false,
            binary: false,
        });
        let record = store
            .prepare_recovery("task", root.path(), "a", original, replacement)
            .unwrap();
        let metadata_hash = record.original_xattrs.clone().unwrap();
        set(b"new attribute");
        assert!(store.apply_recovery("task", &record.id).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"work");
        set(&[0, 255, 3]);
        store.apply_recovery("task", &record.id).unwrap();
        assert_eq!(
            capture_attributes(root.path().to_str().unwrap(), "a").unwrap(),
            attributes
        );
        drop(store);
        let store = ReviewStore::open(data.path().into()).unwrap();
        set(b"later attribute");
        assert!(store.undo_recovery("task", &record.id).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"base");
        set(&[0, 255, 3]);
        store.undo_recovery("task", &record.id).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"work");
        assert_eq!(
            capture_attributes(root.path().to_str().unwrap(), "a").unwrap(),
            attributes
        );
        drop(store);
        fs::File::open(data.path().join("blobs").join(&metadata_hash))
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_hours(24 * 8))
            .unwrap();
        let store = ReviewStore::try_open_maintenance(data.path().into()).unwrap();
        let plan = store.preview_storage_cleanup().unwrap();
        store.execute_storage_cleanup(&plan).unwrap();
        assert_eq!(
            serde_json::from_slice::<super::super::xattrs::ExtendedAttributes>(
                &store.read_blob(&metadata_hash).unwrap()
            )
            .unwrap(),
            attributes
        );
        assert_eq!(store.storage_usage([]).unwrap().missing_references, 0);
    }
    #[cfg(unix)]
    #[test]
    fn recovery_preserves_special_mode_bits_and_refuses_their_drift() {
        use std::os::unix::fs::PermissionsExt;
        for mode in [0o1640, 0o2640, 0o4640] {
            let root = tempfile::tempdir().unwrap();
            let data = tempfile::tempdir().unwrap();
            let store = ReviewStore::open(data.path().into()).unwrap();
            let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
            let path = root.path().join("a");
            fs::write(&path, b"work").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
                mode
            );
            let original = version(&store, &directory, "a").unwrap();
            let replacement = Some(FileVersion {
                hash: store.put_blob(b"base").unwrap(),
                bytes: 4,
                kind: FileKind::File,
                executable: false,
                binary: false,
            });
            let record = store
                .prepare_recovery("task", root.path(), "a", original, replacement)
                .unwrap();
            assert_eq!(record.original_mode, Some(mode));
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            assert!(store.apply_recovery("task", &record.id).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"work");
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            store.apply_recovery("task", &record.id).unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"base");
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
                mode
            );
            drop(store);
            let store = ReviewStore::open(data.path().into()).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            assert!(store.undo_recovery("task", &record.id).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"base");
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            store.undo_recovery("task", &record.id).unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"work");
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
                mode
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn capture_preserves_concurrent_non_executable_permission_changes() {
        use std::os::unix::fs::PermissionsExt;
        for undo in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let data = tempfile::tempdir().unwrap();
            let store = ReviewStore::open(data.path().into()).unwrap();
            let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
            let path = root.path().join("a");
            fs::write(&path, b"work").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            let original = version(&store, &directory, "a").unwrap();
            let replacement = Some(FileVersion {
                hash: store.put_blob(b"base").unwrap(),
                bytes: 4,
                kind: FileKind::File,
                executable: false,
                binary: false,
            });
            let record = store
                .prepare_recovery("task", root.path(), "a", original, replacement)
                .unwrap();
            if undo {
                store.apply_recovery("task", &record.id).unwrap();
            }
            let (expected, desired, phase) = if undo {
                (
                    record.replacement.as_ref(),
                    record.original.as_ref(),
                    "undo",
                )
            } else {
                (
                    record.original.as_ref(),
                    record.replacement.as_ref(),
                    "apply",
                )
            };
            let error = replace_with_pre_capture(&store, &record, expected, desired, phase, || {
                // A chmod after the initial check leaves the content hash and
                // executable bit unchanged, but must not be silently discarded.
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            })
            .unwrap_err();
            assert!(error.contains("permissions changed"));
            assert_eq!(
                fs::read(&path).unwrap(),
                if undo { b"base" } else { b"work" }
            );
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let captured = root
                .path()
                .join(format!(".agent-review-{}-{phase}/captured", record.id));
            assert!(captured.exists());
            assert_eq!(
                fs::metadata(captured).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn actual_replace_undo_and_new_edit_rejection() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        fs::write(root.path().join("a"), "work").unwrap();
        let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        let original = version(&store, &directory, "a").unwrap();
        fs::write(root.path().join("baseline"), "base").unwrap();
        let replacement = version(&store, &directory, "baseline").unwrap();
        let record = store
            .prepare_recovery("task", root.path(), "a", original, replacement)
            .unwrap();
        store.apply_recovery("task", &record.id).unwrap();
        assert_eq!(fs::read(root.path().join("a")).unwrap(), b"base");
        fs::write(root.path().join("a"), "new work").unwrap();
        assert!(store.undo_recovery("task", &record.id).is_err());
        assert_eq!(fs::read(root.path().join("a")).unwrap(), b"new work");
        fs::write(root.path().join("a"), "base").unwrap();
        store.undo_recovery("task", &record.id).unwrap();
        assert_eq!(fs::read(root.path().join("a")).unwrap(), b"work");
    }

    #[test]
    fn deletion_can_be_undone_without_leaving_transaction_files() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        fs::write(root.path().join("new"), "new file").unwrap();
        let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        let record = store
            .prepare_recovery(
                "task",
                root.path(),
                "new",
                version(&store, &directory, "new").unwrap(),
                None,
            )
            .unwrap();
        store.apply_recovery("task", &record.id).unwrap();
        assert!(!root.path().join("new").exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        store.undo_recovery("task", &record.id).unwrap();
        assert_eq!(fs::read(root.path().join("new")).unwrap(), b"new file");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_parent_is_rejected_without_touching_target() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        fs::write(outside.path().join("secret"), "outside").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
        let version = FileVersion {
            hash: store.put_blob(b"outside").unwrap(),
            bytes: 7,
            kind: FileKind::File,
            executable: false,
            binary: false,
        };
        assert!(
            store
                .prepare_recovery("task", root.path(), "link/secret", Some(version), None)
                .is_err()
        );
        assert_eq!(fs::read(outside.path().join("secret")).unwrap(), b"outside");
    }

    #[test]
    fn restores_missing_parent_directories_for_deleted_files_and_undo() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let contents = FileVersion {
            hash: store.put_blob(b"saved").unwrap(),
            bytes: 5,
            kind: FileKind::File,
            executable: false,
            binary: false,
        };
        let restore = store
            .prepare_recovery(
                "task",
                root.path(),
                "missing/nested/file",
                None,
                Some(contents.clone()),
            )
            .unwrap();
        store.apply_recovery("task", &restore.id).unwrap();
        assert_eq!(
            fs::read(root.path().join("missing/nested/file")).unwrap(),
            b"saved"
        );
        let delete = store
            .prepare_recovery(
                "task",
                root.path(),
                "missing/nested/file",
                Some(contents),
                None,
            )
            .unwrap();
        store.apply_recovery("task", &delete.id).unwrap();
        fs::remove_dir(root.path().join("missing/nested")).unwrap();
        fs::remove_dir(root.path().join("missing")).unwrap();
        store.undo_recovery("task", &delete.id).unwrap();
        assert_eq!(
            fs::read(root.path().join("missing/nested/file")).unwrap(),
            b"saved"
        );
    }

    #[test]
    fn reconciles_capture_interruption_and_completed_unmarked_undo() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        fs::write(root.path().join("a"), "work").unwrap();
        fs::write(root.path().join("base"), "base").unwrap();
        let record = store
            .prepare_recovery(
                "task",
                root.path(),
                "a",
                version(&store, &directory, "a").unwrap(),
                version(&store, &directory, "base").unwrap(),
            )
            .unwrap();
        let transaction_name = format!(".agent-review-{}-apply", record.id);
        directory.create_dir(&transaction_name).unwrap();
        let temporary = directory.open_dir(&transaction_name).unwrap();
        directory.rename("a", &temporary, "captured").unwrap();
        fs::write(
            root.path().join(&transaction_name).join("replacement"),
            "base",
        )
        .unwrap();
        drop(temporary);
        let reopened = ReviewStore::open(data.path().into()).unwrap();
        reopened.reconcile_recovery("task", &record.id).unwrap();
        assert_eq!(fs::read(root.path().join("a")).unwrap(), b"base");
        assert!(
            reopened
                .recovery_record("task", &record.id)
                .unwrap()
                .applied
        );
        write_once(
            &data
                .path()
                .join("recovery")
                .join(format!("{}.undo-started", record.id)),
            b"started",
        )
        .unwrap();
        replace(
            &reopened,
            &record,
            record.replacement.as_ref(),
            record.original.as_ref(),
            "undo",
        )
        .unwrap();
        reopened.reconcile_recovery("task", &record.id).unwrap();
        assert!(reopened.recovery_record("task", &record.id).unwrap().undone);
        assert_eq!(fs::read(root.path().join("a")).unwrap(), b"work");
    }

    #[test]
    fn reconciliation_refuses_unknown_current_content() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        fs::write(root.path().join("a"), "work").unwrap();
        let record = store
            .prepare_recovery(
                "task",
                root.path(),
                "a",
                version(&store, &directory, "a").unwrap(),
                None,
            )
            .unwrap();
        fs::write(root.path().join("a"), "external edit").unwrap();
        assert!(store.reconcile_recovery("task", &record.id).is_err());
        assert_eq!(fs::read(root.path().join("a")).unwrap(), b"external edit");
    }

    #[cfg(unix)]
    #[test]
    fn reconciliation_refuses_permission_drift_before_marking_or_cleaning() {
        use std::os::unix::fs::PermissionsExt;
        for captured_phase in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let data = tempfile::tempdir().unwrap();
            let store = ReviewStore::open(data.path().into()).unwrap();
            let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
            fs::write(root.path().join("a"), "private work").unwrap();
            fs::set_permissions(root.path().join("a"), fs::Permissions::from_mode(0o640)).unwrap();
            let record = store
                .prepare_recovery(
                    "task",
                    root.path(),
                    "a",
                    version(&store, &directory, "a").unwrap(),
                    Some(FileVersion {
                        hash: store.put_blob(b"base").unwrap(),
                        bytes: 4,
                        kind: FileKind::File,
                        executable: false,
                        binary: false,
                    }),
                )
                .unwrap();
            let transaction_name = format!(".agent-review-{}-apply", record.id);
            let changed = if captured_phase {
                directory.create_dir(&transaction_name).unwrap();
                let temporary = directory.open_dir(&transaction_name).unwrap();
                directory.rename("a", &temporary, "captured").unwrap();
                root.path().join(&transaction_name).join("captured")
            } else {
                // The replacement was published but its applied marker was not.
                replace(
                    &store,
                    &record,
                    record.original.as_ref(),
                    record.replacement.as_ref(),
                    "apply",
                )
                .unwrap();
                root.path().join("a")
            };
            fs::set_permissions(&changed, fs::Permissions::from_mode(0o644)).unwrap();
            let error = store.reconcile_recovery("task", &record.id).unwrap_err();
            assert!(error.contains("permissions changed"));
            assert!(changed.exists());
            assert!(!store.recovery_record("task", &record.id).unwrap().applied);
            if captured_phase {
                assert!(!root.path().join("a").exists());
            }
            // Correcting only the drift allows the original confirmed transaction to finish.
            fs::set_permissions(&changed, fs::Permissions::from_mode(0o640)).unwrap();
            store.reconcile_recovery("task", &record.id).unwrap();
            assert!(store.recovery_record("task", &record.id).unwrap().applied);
            assert_eq!(fs::read(root.path().join("a")).unwrap(), b"base");
        }
    }

    #[cfg(unix)]
    #[test]
    fn restore_preserves_read_permissions_and_undo_restores_original_mode() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let directory = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        fs::write(root.path().join("a"), "private work").unwrap();
        fs::set_permissions(root.path().join("a"), fs::Permissions::from_mode(0o640)).unwrap();
        let original = version(&store, &directory, "a").unwrap();
        let replacement = FileVersion {
            hash: store.put_blob(b"base").unwrap(),
            bytes: 4,
            kind: FileKind::File,
            executable: false,
            binary: false,
        };
        let record = store
            .prepare_recovery("task", root.path(), "a", original, Some(replacement))
            .unwrap();
        store.apply_recovery("task", &record.id).unwrap();
        assert_eq!(
            fs::metadata(root.path().join("a"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        store.undo_recovery("task", &record.id).unwrap();
        assert_eq!(
            fs::metadata(root.path().join("a"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        assert_eq!(fs::read(root.path().join("a")).unwrap(), b"private work");
    }
}
