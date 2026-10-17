//! Descriptor-bound extended attributes. Apply only to a private replacement
//! candidate; publishing that candidate remains the recovery transaction's job.
use crate::review::ReviewResult;
use rustix::fs::{XattrFlags, fgetxattr, flistxattr, fremovexattr, fsetxattr};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ffi::CString, fs::File};

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_NAMES: usize = 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtendedAttributes {
    // Hex names preserve non-UTF-8 bytes without lossy conversion.
    attributes: BTreeMap<String, Vec<u8>>,
}

fn names(file: &File) -> ReviewResult<Vec<CString>> {
    let length = flistxattr(file, &mut [] as &mut [u8])
        .map_err(|_| "cannot size extended attribute names")?;
    if length > MAX_BYTES {
        return Err("extended attribute names exceed budget".into());
    }
    let mut bytes = vec![0; length];
    let read = flistxattr(file, bytes.as_mut_slice())
        .map_err(|_| "extended attribute names changed or are unreadable")?;
    if read > length {
        return Err("extended attribute names changed".into());
    }
    bytes.truncate(read);
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if bytes.pop() != Some(0) {
        return Err("invalid extended attribute names".into());
    }
    let result = bytes
        .split(|byte| *byte == 0)
        .map(|name| {
            if name.is_empty() {
                return Err("empty extended attribute name".into());
            }
            CString::new(name).map_err(|_| "invalid extended attribute name".into())
        })
        .collect::<ReviewResult<Vec<_>>>()?;
    if result.len() > MAX_NAMES {
        return Err("too many extended attributes".into());
    }
    Ok(result)
}

impl ExtendedAttributes {
    pub fn capture(file: &File) -> ReviewResult<Self> {
        let mut attributes = BTreeMap::new();
        let mut total = 0usize;
        for name in names(file)? {
            let length = fgetxattr(file, name.as_c_str(), &mut [] as &mut [u8])
                .map_err(|_| "cannot size extended attribute")?;
            total = total
                .checked_add(length)
                .and_then(|n| n.checked_add(name.as_bytes().len()))
                .ok_or("extended attribute size overflow")?;
            if total > MAX_BYTES {
                return Err("extended attributes exceed budget".into());
            }
            let mut bytes = vec![0; length];
            let read = fgetxattr(file, name.as_c_str(), bytes.as_mut_slice())
                .map_err(|_| "extended attribute changed or is unreadable")?;
            if read > length {
                return Err("extended attribute changed".into());
            }
            bytes.truncate(read);
            attributes.insert(hex::encode(name.as_bytes()), bytes);
        }
        Ok(Self { attributes })
    }

    pub fn capture_stable(file: &File) -> ReviewResult<Self> {
        let first = Self::capture(file)?;
        if first != Self::capture(file)? {
            return Err("extended attributes changed during capture".into());
        }
        Ok(first)
    }

    pub fn apply_to_candidate(&self, file: &File) -> ReviewResult<()> {
        let metadata = file
            .metadata()
            .map_err(|_| "candidate metadata unavailable")?;
        if !(metadata.is_file()
            || metadata.is_dir()
            || (cfg!(target_os = "macos") && metadata.file_type().is_symlink()))
        {
            return Err("unsupported extended attribute candidate type".into());
        }
        // Validate deserialized data before changing even the private candidate.
        if self.attributes.len() > MAX_NAMES {
            return Err("too many extended attributes".into());
        }
        let mut total = 0usize;
        let decoded = self
            .attributes
            .iter()
            .map(|(name, value)| {
                let raw = hex::decode(name).map_err(|_| "invalid encoded attribute name")?;
                if raw.is_empty() {
                    return Err("empty extended attribute name".into());
                }
                total = total
                    .checked_add(raw.len())
                    .and_then(|n| n.checked_add(value.len()))
                    .ok_or("extended attribute size overflow")?;
                if total > MAX_BYTES {
                    return Err("extended attributes exceed budget".into());
                }
                Ok((
                    CString::new(raw).map_err(|_| "invalid attribute name")?,
                    value,
                ))
            })
            .collect::<ReviewResult<Vec<_>>>()?;
        for name in names(file)? {
            if !self.attributes.contains_key(&hex::encode(name.as_bytes())) {
                fremovexattr(file, name.as_c_str())
                    .map_err(|_| "cannot remove candidate extended attribute")?;
            }
        }
        for (name, value) in decoded {
            fsetxattr(file, name.as_c_str(), value, XattrFlags::empty())
                .map_err(|_| "cannot restore candidate extended attribute")?;
        }
        file.sync_all()
            .map_err(|_| "cannot persist candidate attributes")?;
        if self != &Self::capture_stable(file)? {
            return Err("candidate extended attribute verification failed".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptor_attributes_roundtrip_binary_empty_and_replaced_paths() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source");
        let source = File::create(&path).unwrap();
        for (name, value) in [
            ("user.dogcodes.binary", &[0, 255, 3][..]),
            ("user.dogcodes.empty", &[][..]),
        ] {
            fsetxattr(&source, name, value, XattrFlags::empty()).unwrap();
        }
        let saved = ExtendedAttributes::capture_stable(&source).unwrap();
        std::fs::rename(&path, directory.path().join("moved")).unwrap();
        File::create(&path).unwrap();
        assert_eq!(ExtendedAttributes::capture_stable(&source).unwrap(), saved);
        let candidate = File::create(directory.path().join("candidate")).unwrap();
        fsetxattr(
            &candidate,
            "user.dogcodes.extra",
            b"remove",
            XattrFlags::empty(),
        )
        .unwrap();
        let restored: ExtendedAttributes =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        restored.apply_to_candidate(&candidate).unwrap();
        assert_eq!(
            ExtendedAttributes::capture_stable(&candidate).unwrap(),
            saved
        );
        let invalid: ExtendedAttributes =
            serde_json::from_str(r#"{"attributes":{"00":[1]}}"#).unwrap();
        assert!(invalid.apply_to_candidate(&candidate).is_err());
        assert_eq!(
            ExtendedAttributes::capture_stable(&candidate).unwrap(),
            saved
        );
    }
}
