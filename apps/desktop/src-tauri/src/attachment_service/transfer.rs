//! Durable, bounded upload chunks; only a verified commit publishes an attachment.
use super::{
    Arc, AttachmentStore, AttachmentUploadRequest, Engine, MAX_BYTES, Manifest, Path, PathBuf,
    PromptAttachment, Read, STANDARD, Write, fs,
};
use serde_json::{Value, json};
const CHUNK_BYTES: usize = 256 * 1024;

impl AttachmentStore {
    fn staging(&self, request: &AttachmentUploadRequest, create: bool) -> Result<PathBuf, String> {
        Self::validate(request)?;
        let root = self.root(create)?.ok_or("backendText.m95eeb5e5")?;
        let uploads = root.join("transfers");
        if create {
            fs::create_dir_all(&uploads).map_err(|_| "backendText.m2647bcd8")?;
        }
        let uploads =
            Self::checked_child(&root, Path::new("transfers"))?.ok_or("backendText.m95eeb5e5")?;
        let directory = uploads.join(&request.upload_id);
        if create && !directory.exists() {
            let mut count = 0;
            let mut reserved = 0_u64;
            for entry in fs::read_dir(&uploads).map_err(|_| "backendText.meae6c73c")? {
                let entry = entry.map_err(|_| "backendText.meae6c73c")?;
                if entry.file_name().to_string_lossy().starts_with(".pending-") {
                    if entry
                        .file_type()
                        .map_err(|_| "backendText.meae6c73c")?
                        .is_dir()
                    {
                        let _ = fs::remove_dir_all(entry.path());
                    }
                    continue;
                }
                if entry
                    .file_type()
                    .map_err(|_| "backendText.meae6c73c")?
                    .is_dir()
                {
                    count += 1;
                    let bytes = Self::read_file(&entry.path().join("manifest.json"), 4096)?;
                    let manifest: Manifest =
                        serde_json::from_slice(&bytes).map_err(|_| "backendText.m51d996e7")?;
                    reserved = reserved.saturating_add(manifest.request.byte_length);
                }
            }
            if count >= 32 || reserved.saturating_add(request.byte_length) > 256 * 1024 * 1024 {
                return Err("backendText.m4b554f6b".into());
            }
            let staging = tempfile::Builder::new()
                .prefix(".pending-")
                .tempdir_in(&uploads)
                .map_err(|_| "backendText.me13c8e50")?;
            let mut manifest = fs::File::create(staging.path().join("manifest.json"))
                .map_err(|_| "backendText.m921a5a89")?;
            serde_json::to_writer(
                &mut manifest,
                &Manifest {
                    schema_version: 1,
                    request: request.clone(),
                },
            )
            .map_err(|_| "backendText.m921a5a89")?;
            manifest.sync_all().map_err(|_| "backendText.m3f2978ba")?;
            // Windows cannot rename the directory while its manifest is open.
            drop(manifest);
            fs::File::create(staging.path().join("bytes"))
                .and_then(|f| f.sync_all())
                .map_err(|_| "backendText.mc4b03780")?;
            fs::rename(staging.path(), &directory).map_err(|_| "backendText.m921a5a89")?;
        }
        let directory =
            Self::checked_child(&root, &Path::new("transfers").join(&request.upload_id))?
                .ok_or("backendText.m95eeb5e5")?;
        let manifest: Manifest =
            serde_json::from_slice(&Self::read_file(&directory.join("manifest.json"), 4096)?)
                .map_err(|_| "backendText.m51d996e7")?;
        if manifest.request != *request || manifest.schema_version != 1 {
            return Err("backendText.m5efccc24".into());
        }
        Ok(directory)
    }
    pub fn transfer_status(
        &self,
        request: &AttachmentUploadRequest,
        create: bool,
    ) -> Result<Value, String> {
        let _guard = self.gate.lock().map_err(|_| "backendText.mf6348b90")?;
        Self::validate(request)?;
        if let Some(root) = self.root(false)?
            && let Some(receipt) = Self::receipt_at(&root, request)?
        {
            return Ok(
                json!({"state":"completed","offset":request.byte_length.to_string(),"attachment":receipt}),
            );
        }
        let directory = self.staging(request, create)?;
        let path =
            Self::checked_child(&directory, Path::new("bytes"))?.ok_or("backendText.m0341967e")?;
        let size = fs::metadata(path)
            .map_err(|_| "backendText.m006a307a")?
            .len();
        if size > request.byte_length {
            return Err("backendText.mbdfa4c97".into());
        }
        Ok(json!({"state":"uploading","offset":size.to_string(),"maxChunkBytes":CHUNK_BYTES}))
    }
    pub fn transfer_chunk(
        &self,
        request: &AttachmentUploadRequest,
        offset: u64,
        encoded: &str,
    ) -> Result<Value, String> {
        use std::io::{Seek, SeekFrom};
        if encoded.len() > CHUNK_BYTES.div_ceil(3) * 4 {
            return Err("backendText.m4a36501c".into());
        }
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| "backendText.m3e7c8aa9")?;
        if bytes.is_empty() || bytes.len() > CHUNK_BYTES {
            return Err("backendText.me7c0f6ce".into());
        }
        let _guard = self.gate.lock().map_err(|_| "backendText.mf6348b90")?;
        let directory = self.staging(request, false)?;
        let path =
            Self::checked_child(&directory, Path::new("bytes"))?.ok_or("backendText.m0341967e")?;
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| "backendText.m006a307a")?;
        let length = file.metadata().map_err(|_| "backendText.m006a307a")?.len();
        let end = offset
            .checked_add(bytes.len() as u64)
            .filter(|end| *end <= request.byte_length)
            .ok_or("backendText.mdc4f7078")?;
        if offset > length {
            return Err("backendText.m3e78f372".into());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| "backendText.mcfe3c452")?;
        if offset < length {
            if end > length {
                return Err("backendText.m297d325d".into());
            }
            let mut previous = vec![0; bytes.len()];
            file.read_exact(&mut previous)
                .map_err(|_| "backendText.m062baa6a")?;
            if previous != bytes {
                return Err("backendText.m6182215a".into());
            }
        } else {
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| "backendText.mbf74561e")?;
        }
        Ok(json!({"offset":length.max(end).to_string()}))
    }
    pub fn transfer_commit(
        &self,
        request: &AttachmentUploadRequest,
    ) -> Result<PromptAttachment, String> {
        if let Some(receipt) = self.receipt(request)? {
            return Ok(receipt);
        }
        let (directory, bytes) = {
            let _guard = self.gate.lock().map_err(|_| "backendText.mf6348b90")?;
            let directory = self.staging(request, false)?;
            let path = Self::checked_child(&directory, Path::new("bytes"))?
                .ok_or("backendText.m0341967e")?;
            let bytes = Self::read_file(&path, MAX_BYTES)?;
            (directory, bytes)
        };
        let reference = self.save(request, &STANDARD.encode(bytes))?;
        let _guard = self.gate.lock().map_err(|_| "backendText.mf6348b90")?;
        let _ = fs::remove_dir_all(directory);
        Ok(reference)
    }
    pub fn transfer_abort(&self, request: &AttachmentUploadRequest) -> Result<(), String> {
        let _guard = self.gate.lock().map_err(|_| "backendText.mf6348b90")?;
        Self::validate(request)?;
        let Some(root) = self.root(false)? else {
            return Ok(());
        };
        if Self::checked_child(&root, &Path::new("transfers").join(&request.upload_id))?.is_none() {
            return Ok(());
        }
        let directory = self.staging(request, false)?;
        fs::remove_dir_all(directory).map_err(|_| "backendText.mf6d7aad4")?;
        Ok(())
    }
}

pub(crate) async fn dispatch(
    store: Arc<AttachmentStore>,
    method: &str,
    mut args: Value,
) -> Result<Value, String> {
    let method = method.to_string();
    if (method == "attachment_upload_chunk" || method == "attachment_read_chunk")
        && let Some(raw) = args.get(1).and_then(Value::as_str)
    {
        args[1] = json!(raw.parse::<u64>().map_err(|_| "backendText.me6608ed2")?);
    }
    tokio::task::spawn_blocking(move || match method.as_str() {
        "attachment_upload_begin"|"attachment_upload_status"=>{
            let request=serde_json::from_value(args).map_err(|e|e.to_string())?;
            store.transfer_status(&request,method=="attachment_upload_begin")
        }
        "attachment_upload_chunk"=>{
            let (request,offset,encoded):(AttachmentUploadRequest,u64,String)=serde_json::from_value(args).map_err(|e|e.to_string())?;
            store.transfer_chunk(&request,offset,&encoded)
        }
        "attachment_upload_commit"=>{
            let request=serde_json::from_value(args).map_err(|e|e.to_string())?;
            Ok(json!(store.transfer_commit(&request)?))
        }
        "attachment_upload_abort"=>{
            let request=serde_json::from_value(args).map_err(|e|e.to_string())?;
            store.transfer_abort(&request)?; Ok(Value::Null)
        }
        "attachment_read_chunk"=>{
            let (path,offset,limit):(String,u64,usize)=serde_json::from_value(args).map_err(|e|e.to_string())?;
            if limit==0 || limit>CHUNK_BYTES {return Err("backendText.me7c0f6ce".into());}
            let bytes=STANDARD.decode(store.read(&path)?).map_err(|_|"backendText.m3e7c8aa9")?;
            let offset=usize::try_from(offset).map_err(|_|"backendText.me6608ed2")?;
            if offset>bytes.len() {return Err("backendText.m58060d8b".into());}
            let end=offset.saturating_add(limit).min(bytes.len());
            Ok(json!({"bytes":STANDARD.encode(&bytes[offset..end]),"next":end.to_string(),"eof":end==bytes.len()}))
        }
        _=>Err("backendText.m8a17506f".into()),
    }).await.map_err(|_|"backendText.mf333a656".to_string())?
}
