//! Managed attachment copies, owned by the local service rather than a window.
use agent_domain::{AttachmentIntegrity, PromptAttachment};
use agent_protocol::AttachmentUploadRequest;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub const MAX_BYTES: usize = 25 * 1024 * 1024;
const MAX_BASE64: usize = MAX_BYTES.div_ceil(3) * 4;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u8,
    request: AttachmentUploadRequest,
}

pub struct AttachmentStore {
    root: PathBuf,
    gate: Mutex<()>,
}
impl AttachmentStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            gate: Mutex::new(()),
        }
    }
    fn validate(request: &AttachmentUploadRequest) -> Result<(), String> {
        if uuid::Uuid::parse_str(&request.upload_id).is_err()
            || uuid::Uuid::parse_str(&request.upload_id)
                .is_ok_and(|id| id.to_string() != request.upload_id)
            || request.name.is_empty()
            || request.name.len() > 512
            || request.name.chars().any(char::is_control)
            || request.mime_type.is_empty()
            || request.mime_type.len() > 128
            || request.mime_type.chars().any(char::is_control)
            || request.byte_length > MAX_BYTES as u64
            || request.sha256.len() != 64
            || !request
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("附件上传身份或元数据无效".into());
        }
        Ok(())
    }
    fn filename(request: &AttachmentUploadRequest) -> String {
        // The original name is metadata. Storage paths are portable and never
        // contain user-provided directory components or Windows reserved names.
        let extension = request
            .name
            .rsplit_once('.')
            .map(|(_, ext)| ext)
            .filter(|ext| {
                !ext.is_empty() && ext.len() <= 12 && ext.bytes().all(|b| b.is_ascii_alphanumeric())
            });
        extension.map_or_else(
            || "payload".into(),
            |ext| format!("payload.{}", ext.to_ascii_lowercase()),
        )
    }
    fn reference(directory: &Path, request: &AttachmentUploadRequest) -> PromptAttachment {
        PromptAttachment {
            name: request.name.clone(),
            mime_type: request.mime_type.clone(),
            path: directory
                .join(Self::filename(request))
                .to_string_lossy()
                .into_owned(),
            integrity: Some(AttachmentIntegrity {
                sha256: request.sha256.clone(),
                byte_length: request.byte_length,
            }),
        }
    }
    fn root(&self, create: bool) -> Result<Option<PathBuf>, String> {
        if create {
            fs::create_dir_all(&self.root).map_err(|_| "无法创建附件目录")?;
        }
        match fs::canonicalize(&self.root) {
            Ok(path) => Ok(Some(path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => Ok(None),
            Err(_) => Err("附件目录不可用".into()),
        }
    }
    fn checked_child(root: &Path, relative: &Path) -> Result<Option<PathBuf>, String> {
        let candidate = root.join(relative);
        let metadata = match fs::symlink_metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("附件记录不可读取".into()),
        };
        if metadata.file_type().is_symlink() {
            return Err("附件路径已变化".into());
        }
        let path = fs::canonicalize(candidate).map_err(|_| "附件路径不可读取")?;
        if !path.starts_with(root) {
            return Err("不是应用保存的附件".into());
        }
        Ok(Some(path))
    }
    fn read_file(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
        let metadata = fs::symlink_metadata(path).map_err(|_| "附件已不可读取")?;
        if !metadata.is_file() || metadata.len() > limit as u64 {
            return Err("附件文件无效或过大".into());
        }
        let file = fs::File::open(path).map_err(|_| "附件已不可读取")?;
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "附件读取失败")?;
        if bytes.len() > limit {
            return Err("附件文件过大".into());
        }
        Ok(bytes)
    }
    fn receipt_at(
        root: &Path,
        request: &AttachmentUploadRequest,
    ) -> Result<Option<PromptAttachment>, String> {
        let Some(directory) =
            Self::checked_child(root, &Path::new("uploads").join(&request.upload_id))?
        else {
            return Ok(None);
        };
        let manifest: Manifest =
            serde_json::from_slice(&Self::read_file(&directory.join("manifest.json"), 4096)?)
                .map_err(|_| "附件回执损坏，请保留原记录")?;
        if manifest.schema_version != 1 || manifest.request != *request {
            return Err("原上传身份已绑定不同的附件，未覆盖文件".into());
        }
        let reference = Self::reference(&directory, request);
        let bytes = Self::read_file(Path::new(&reference.path), MAX_BYTES)?;
        driver_api::attachments::verify_bytes(&reference, &bytes)
            .map_err(|error| error.to_string())?;
        Ok(Some(reference))
    }
    pub fn receipt(
        &self,
        request: &AttachmentUploadRequest,
    ) -> Result<Option<PromptAttachment>, String> {
        Self::validate(request)?;
        let _guard = self.gate.lock().map_err(|_| "附件服务暂时不可用")?;
        let Some(root) = self.root(false)? else {
            return Ok(None);
        };
        Self::receipt_at(&root, request)
    }
    pub fn save(
        &self,
        request: &AttachmentUploadRequest,
        encoded: &str,
    ) -> Result<PromptAttachment, String> {
        Self::validate(request)?;
        if encoded.len() > MAX_BASE64 {
            return Err("附件超过 25 MB".into());
        }
        let bytes = STANDARD.decode(encoded).map_err(|_| "附件编码无效")?;
        let reference = Self::reference(Path::new("unused"), request);
        driver_api::attachments::verify_bytes(&reference, &bytes)
            .map_err(|error| error.to_string())?;
        let _guard = self.gate.lock().map_err(|_| "附件服务暂时不可用")?;
        let root = self.root(true)?.ok_or("附件目录不可用")?;
        if let Some(receipt) = Self::receipt_at(&root, request)? {
            return Ok(receipt);
        }
        let uploads = root.join("uploads");
        fs::create_dir_all(&uploads).map_err(|_| "无法创建上传目录")?;
        let uploads = Self::checked_child(&root, Path::new("uploads"))?.ok_or("上传目录不可用")?;
        // Only a fully written and synced directory is published. Error paths
        // remove the staging directory; a process crash may leave an unreferenced
        // .pending directory, never a successful receipt or partially sent file.
        let staging = tempfile::Builder::new()
            .prefix(".pending-")
            .tempdir_in(&uploads)
            .map_err(|_| "无法准备附件")?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staging.path().join(Self::filename(request)))
            .map_err(|_| "无法保存附件")?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| "附件写入失败")?;
        let manifest = serde_json::to_vec(&Manifest {
            schema_version: 1,
            request: request.clone(),
        })
        .map_err(|_| "附件回执编码失败")?;
        let mut receipt = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staging.path().join("manifest.json"))
            .map_err(|_| "无法保存附件回执")?;
        receipt
            .write_all(&manifest)
            .and_then(|()| receipt.sync_all())
            .map_err(|_| "附件回执写入失败")?;
        drop(file);
        drop(receipt);
        #[cfg(unix)]
        fs::File::open(staging.path())
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "附件目录同步失败")?;
        let destination = uploads.join(&request.upload_id);
        if fs::rename(staging.path(), &destination).is_err() {
            // Another service instance may have won publication. Never replace
            // an existing object or its metadata to resolve that race.
            return Self::receipt_at(&root, request)?.ok_or("附件发布失败，请核实原上传".into());
        }
        #[cfg(unix)]
        fs::File::open(&uploads)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "附件发布结果尚未确认，请核实原上传")?;
        Self::receipt_at(&root, request)?.ok_or("附件回执尚未可用".into())
    }
    pub fn read(&self, path: &str) -> Result<String, String> {
        let _guard = self.gate.lock().map_err(|_| "附件服务暂时不可用")?;
        let root = self.root(false)?.ok_or("附件已不可读取")?;
        let candidate = Path::new(path);
        if !candidate.is_absolute() {
            return Err("附件路径无效".into());
        }
        if fs::symlink_metadata(candidate)
            .map_err(|_| "附件已不可读取")?
            .file_type()
            .is_symlink()
        {
            return Err("附件路径已变化".into());
        }
        let canonical = fs::canonicalize(candidate).map_err(|_| "附件已不可读取")?;
        if !canonical.starts_with(&root) {
            return Err("不是应用保存的附件".into());
        }
        // Keep a managed path managed even if an ancestor was replaced with a
        // link to a legacy directory. It must still pass its original receipt.
        let relative = candidate
            .strip_prefix(&root)
            .or_else(|_| canonical.strip_prefix(&root))
            .map_err(|_| "附件路径无效")?;
        let parts: Vec<_> = relative.components().collect();
        if parts
            .first()
            .is_some_and(|part| part.as_os_str() == "uploads")
        {
            if parts.len() != 3 {
                return Err("附件路径无效".into());
            }
            let manifest: Manifest = serde_json::from_slice(&Self::read_file(
                &canonical
                    .parent()
                    .ok_or("附件路径无效")?
                    .join("manifest.json"),
                4096,
            )?)
            .map_err(|_| "附件回执损坏")?;
            Self::validate(&manifest.request)?;
            let reference = Self::receipt_at(&root, &manifest.request)?.ok_or("附件回执不存在")?;
            if Path::new(&reference.path) != canonical {
                return Err("附件引用与回执不一致".into());
            }
            let bytes = Self::read_file(&canonical, MAX_BYTES)?;
            driver_api::attachments::verify_bytes(&reference, &bytes)
                .map_err(|error| error.to_string())?;
            return Ok(STANDARD.encode(bytes));
        }
        // Existing UUID directories predate manifests; preserve their history.
        if parts.len() != 2
            || parts[0]
                .as_os_str()
                .to_str()
                .is_none_or(|id| uuid::Uuid::parse_str(id).is_err())
        {
            return Err("不是已保存的附件文件".into());
        }
        Ok(STANDARD.encode(Self::read_file(&canonical, MAX_BYTES)?))
    }
}

pub async fn dispatch(
    store: Arc<AttachmentStore>,
    method: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let method = method.to_owned();
    tokio::task::spawn_blocking(move || match method.as_str() {
        "attachment_save" => {
            let (request, encoded): (AttachmentUploadRequest, String) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(store.save(&request, &encoded)?).map_err(|e| e.to_string())
        }
        "attachment_receipt" => {
            let request = serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(store.receipt(&request)?).map_err(|e| e.to_string())
        }
        "attachment_read" => {
            let path: String = serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(store.read(&path)?).map_err(|e| e.to_string())
        }
        _ => Err("未知附件操作".into()),
    })
    .await
    .map_err(|_| "附件服务执行失败".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    fn request(bytes: &[u8]) -> AttachmentUploadRequest {
        AttachmentUploadRequest {
            upload_id: uuid::Uuid::new_v4().to_string(),
            name: "notes.txt".into(),
            mime_type: "text/plain".into(),
            byte_length: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        }
    }
    #[test]
    fn attachment_receipt_is_read_only_and_reopens_the_same_upload() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("attachments");
        let store = AttachmentStore::new(root.clone());
        let request = request(b"original");
        assert!(store.receipt(&request).unwrap().is_none());
        assert!(!root.exists());
        let first = store.save(&request, &STANDARD.encode(b"original")).unwrap();
        let reopened = AttachmentStore::new(root.clone());
        assert_eq!(reopened.receipt(&request).unwrap(), Some(first.clone()));
        assert_eq!(
            reopened
                .save(&request, &STANDARD.encode(b"original"))
                .unwrap(),
            first
        );
        assert_eq!(fs::read_dir(root.join("uploads")).unwrap().count(), 1);
        assert_eq!(
            STANDARD
                .decode(reopened.read(&first.path).unwrap())
                .unwrap(),
            b"original"
        );
    }
    #[test]
    fn attachment_identity_binds_name_type_size_and_digest_without_overwriting() {
        let temp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(temp.path().into());
        let request = request(b"original");
        let original = store.save(&request, &STANDARD.encode(b"original")).unwrap();
        for field in 0..4 {
            let mut changed = request.clone();
            match field {
                0 => changed.name = "other.txt".into(),
                1 => changed.mime_type = "image/png".into(),
                2 => changed.byte_length += 1,
                _ => changed.sha256 = "0".repeat(64),
            }
            assert!(store.receipt(&changed).is_err());
            assert!(store.save(&changed, &STANDARD.encode(b"original")).is_err());
        }
        assert_eq!(fs::read(original.path).unwrap(), b"original");
    }
    #[test]
    fn attachment_bytes_must_match_before_any_publication() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("attachments");
        let store = AttachmentStore::new(root.clone());
        let request = request(b"original");
        assert!(store.save(&request, &STANDARD.encode(b"modified")).is_err());
        assert!(store.save(&request, "***").is_err());
        assert!(!root.exists());
        assert!(store.receipt(&request).unwrap().is_none());
    }
    #[test]
    fn attachment_tampering_and_missing_payload_do_not_rewrite_the_original() {
        let temp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(temp.path().into());
        let request = request(b"original");
        let original = store.save(&request, &STANDARD.encode(b"original")).unwrap();
        fs::write(&original.path, b"modified").unwrap();
        assert!(store.receipt(&request).is_err());
        assert!(store.read(&original.path).is_err());
        assert!(store.save(&request, &STANDARD.encode(b"original")).is_err());
        assert_eq!(fs::read(&original.path).unwrap(), b"modified");
        fs::remove_file(&original.path).unwrap();
        assert!(store.receipt(&request).is_err());
        assert!(store.save(&request, &STANDARD.encode(b"original")).is_err());
    }
    #[test]
    fn attachment_paths_are_portable_and_invalid_ids_cannot_escape() {
        let temp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(temp.path().into());
        let mut request = request(b"file");
        request.name = "../../CON.txt".into();
        let saved = store.save(&request, &STANDARD.encode(b"file")).unwrap();
        assert_eq!(Path::new(&saved.path).file_name().unwrap(), "payload.txt");
        for id in [
            "../outside",
            "",
            "not-an-id",
            "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA",
        ] {
            request.upload_id = id.into();
            assert!(store.receipt(&request).is_err());
            assert!(store.save(&request, &STANDARD.encode(b"file")).is_err());
        }
        assert!(
            store
                .read(temp.path().parent().unwrap().to_str().unwrap())
                .is_err()
        );
    }
    #[test]
    fn attachment_legacy_files_remain_readable_but_staging_and_manifests_are_not_files() {
        let temp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(temp.path().into());
        let legacy = temp.path().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join("old.png"), b"old").unwrap();
        assert_eq!(
            STANDARD
                .decode(
                    store
                        .read(legacy.join("old.png").to_str().unwrap())
                        .unwrap()
                )
                .unwrap(),
            b"old"
        );
        let request = request(b"file");
        let saved = store.save(&request, &STANDARD.encode(b"file")).unwrap();
        let manifest = Path::new(&saved.path)
            .parent()
            .unwrap()
            .join("manifest.json");
        assert!(store.read(manifest.to_str().unwrap()).is_err());
        let stage = temp.path().join("uploads/.pending-crashed");
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("payload"), b"partial").unwrap();
        assert!(store.read(stage.join("payload").to_str().unwrap()).is_err());
    }
    #[test]
    fn attachment_publication_race_uses_one_original_object() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let request = request(b"file");
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let root = root.clone();
                let request = request.clone();
                std::thread::spawn(move || {
                    AttachmentStore::new(root).save(&request, &STANDARD.encode(b"file"))
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().unwrap())
            .collect();
        assert!(results.iter().all(|result| result == &results[0]));
        assert_eq!(fs::read_dir(root.join("uploads")).unwrap().count(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn attachment_symlinks_cannot_publish_or_read_outside_the_store() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("uploads")).unwrap();
        let store = AttachmentStore::new(temp.path().into());
        let request = request(b"file");
        assert!(store.save(&request, &STANDARD.encode(b"file")).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        let path = outside.path().join("private");
        fs::write(&path, b"secret").unwrap();
        assert!(store.read(path.to_str().unwrap()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn managed_attachment_cannot_fall_back_to_legacy_after_directory_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(temp.path().into());
        let request = request(b"original");
        let saved = store.save(&request, &STANDARD.encode(b"original")).unwrap();
        let directory = Path::new(&saved.path).parent().unwrap();
        let legacy = temp.path().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join("payload.txt"), b"modified").unwrap();
        fs::remove_dir_all(directory).unwrap();
        std::os::unix::fs::symlink(&legacy, directory).unwrap();
        assert!(store.receipt(&request).is_err());
        assert!(store.read(&saved.path).is_err());
    }
}
