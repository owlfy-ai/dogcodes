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
        let root = self.root(create)?.ok_or("上传不存在")?;
        let uploads = root.join("transfers");
        if create {
            fs::create_dir_all(&uploads).map_err(|_| "无法创建上传目录")?;
        }
        let uploads = Self::checked_child(&root, Path::new("transfers"))?.ok_or("上传不存在")?;
        let directory = uploads.join(&request.upload_id);
        if create && !directory.exists() {
            let mut count = 0;
            let mut reserved = 0_u64;
            for entry in fs::read_dir(&uploads).map_err(|_| "无法读取上传目录")? {
                let entry = entry.map_err(|_| "无法读取上传目录")?;
                if entry.file_name().to_string_lossy().starts_with(".pending-") {
                    if entry.file_type().map_err(|_| "无法读取上传目录")?.is_dir() {
                        let _ = fs::remove_dir_all(entry.path());
                    }
                    continue;
                }
                if entry.file_type().map_err(|_| "无法读取上传目录")?.is_dir() {
                    count += 1;
                    let bytes = Self::read_file(&entry.path().join("manifest.json"), 4096)?;
                    let manifest: Manifest =
                        serde_json::from_slice(&bytes).map_err(|_| "上传记录损坏")?;
                    reserved = reserved.saturating_add(manifest.request.byte_length);
                }
            }
            if count >= 32 || reserved.saturating_add(request.byte_length) > 256 * 1024 * 1024 {
                return Err("未完成的附件过多，请先完成或取消其他上传".into());
            }
            let staging = tempfile::Builder::new()
                .prefix(".pending-")
                .tempdir_in(&uploads)
                .map_err(|_| "无法准备附件上传")?;
            let mut manifest = fs::File::create(staging.path().join("manifest.json"))
                .map_err(|_| "无法保存上传身份")?;
            serde_json::to_writer(
                &mut manifest,
                &Manifest {
                    schema_version: 1,
                    request: request.clone(),
                },
            )
            .map_err(|_| "无法保存上传身份")?;
            manifest.sync_all().map_err(|_| "无法同步上传身份")?;
            // Windows cannot rename the directory while its manifest is open.
            drop(manifest);
            fs::File::create(staging.path().join("bytes"))
                .and_then(|f| f.sync_all())
                .map_err(|_| "无法创建上传文件")?;
            fs::rename(staging.path(), &directory).map_err(|_| "无法保存上传身份")?;
        }
        let directory =
            Self::checked_child(&root, &Path::new("transfers").join(&request.upload_id))?
                .ok_or("上传不存在")?;
        let manifest: Manifest =
            serde_json::from_slice(&Self::read_file(&directory.join("manifest.json"), 4096)?)
                .map_err(|_| "上传记录损坏")?;
        if manifest.request != *request || manifest.schema_version != 1 {
            return Err("原上传身份已绑定不同的附件".into());
        }
        Ok(directory)
    }
    pub fn transfer_status(
        &self,
        request: &AttachmentUploadRequest,
        create: bool,
    ) -> Result<Value, String> {
        let _guard = self.gate.lock().map_err(|_| "附件服务暂时不可用")?;
        Self::validate(request)?;
        if let Some(root) = self.root(false)?
            && let Some(receipt) = Self::receipt_at(&root, request)?
        {
            return Ok(
                json!({"state":"completed","offset":request.byte_length.to_string(),"attachment":receipt}),
            );
        }
        let directory = self.staging(request, create)?;
        let path = Self::checked_child(&directory, Path::new("bytes"))?.ok_or("上传文件不存在")?;
        let size = fs::metadata(path).map_err(|_| "上传文件不可读取")?.len();
        if size > request.byte_length {
            return Err("上传文件长度异常".into());
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
            return Err("附件分块过大".into());
        }
        let bytes = STANDARD.decode(encoded).map_err(|_| "附件编码无效")?;
        if bytes.is_empty() || bytes.len() > CHUNK_BYTES {
            return Err("附件分块大小无效".into());
        }
        let _guard = self.gate.lock().map_err(|_| "附件服务暂时不可用")?;
        let directory = self.staging(request, false)?;
        let path = Self::checked_child(&directory, Path::new("bytes"))?.ok_or("上传文件不存在")?;
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| "上传文件不可读取")?;
        let length = file.metadata().map_err(|_| "上传文件不可读取")?.len();
        let end = offset
            .checked_add(bytes.len() as u64)
            .filter(|end| *end <= request.byte_length)
            .ok_or("附件分块超过声明长度")?;
        if offset > length {
            return Err("附件分块存在缺口，请重新核实上传位置".into());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| "无法定位上传位置")?;
        if offset < length {
            if end > length {
                return Err("附件分块与原上传位置冲突".into());
            }
            let mut previous = vec![0; bytes.len()];
            file.read_exact(&mut previous)
                .map_err(|_| "无法核实原附件分块")?;
            if previous != bytes {
                return Err("附件分块与原内容不同，未覆盖".into());
            }
        } else {
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| "附件分块保存失败，请核实位置")?;
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
            let _guard = self.gate.lock().map_err(|_| "附件服务暂时不可用")?;
            let directory = self.staging(request, false)?;
            let path =
                Self::checked_child(&directory, Path::new("bytes"))?.ok_or("上传文件不存在")?;
            let bytes = Self::read_file(&path, MAX_BYTES)?;
            (directory, bytes)
        };
        let reference = self.save(request, &STANDARD.encode(bytes))?;
        let _guard = self.gate.lock().map_err(|_| "附件服务暂时不可用")?;
        let _ = fs::remove_dir_all(directory);
        Ok(reference)
    }
    pub fn transfer_abort(&self, request: &AttachmentUploadRequest) -> Result<(), String> {
        let _guard = self.gate.lock().map_err(|_| "附件服务暂时不可用")?;
        Self::validate(request)?;
        let Some(root) = self.root(false)? else {
            return Ok(());
        };
        if Self::checked_child(&root, &Path::new("transfers").join(&request.upload_id))?.is_none() {
            return Ok(());
        }
        let directory = self.staging(request, false)?;
        fs::remove_dir_all(directory).map_err(|_| "无法取消上传")?;
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
        args[1] = json!(raw.parse::<u64>().map_err(|_| "无效附件位置")?);
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
            if limit==0 || limit>CHUNK_BYTES {return Err("附件分块大小无效".into());}
            let bytes=STANDARD.decode(store.read(&path)?).map_err(|_|"附件编码无效")?;
            let offset=usize::try_from(offset).map_err(|_|"无效附件位置")?;
            if offset>bytes.len() {return Err("附件位置超过内容长度".into());}
            let end=offset.saturating_add(limit).min(bytes.len());
            Ok(json!({"bytes":STANDARD.encode(&bytes[offset..end]),"next":end.to_string(),"eof":end==bytes.len()}))
        }
        _=>Err("未知附件传输操作".into()),
    }).await.map_err(|_|"附件传输执行失败".to_string())?
}
