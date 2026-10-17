use agent_core::turn_timing::{TurnTimingRecord, TurnTimingSink};
use std::{io::Write, path::Path, sync::Arc};

/// Bounded, non-blocking diagnostics: never queue file I/O behind event writes.
pub(crate) fn file_sink(directory: &Path) -> TurnTimingSink {
    let path = directory.join("turn-startup-timing.jsonl");
    let (sender, receiver) = std::sync::mpsc::sync_channel::<TurnTimingRecord>(1024);
    let _ = std::thread::Builder::new()
        .name("turn-timing".into())
        .spawn(move || {
            while let Ok(record) = receiver.recv() {
                if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > 2 * 1024 * 1024) {
                    let _ = std::fs::rename(&path, path.with_extension("previous.jsonl"));
                }
                let mut options = std::fs::OpenOptions::new();
                options.create(true).append(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                if let Ok(mut file) = options.open(&path)
                    && let Ok(mut line) = serde_json::to_vec(&record)
                {
                    line.push(b'\n');
                    let _ = file.write_all(&line);
                }
            }
        });
    Arc::new(move |record| {
        let _ = sender.try_send(record);
    })
}
