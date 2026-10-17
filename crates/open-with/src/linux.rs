use super::*;
use gio::prelude::*;

fn handlers(path: &Path) -> Result<(Vec<gio::AppInfo>, Option<gio::AppInfo>), String> {
    let file = gio::File::for_path(path);
    let info = file
        .query_info(
            "standard::content-type",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .map_err(|_| "无法识别文件类型")?;
    let content_type = info.content_type().ok_or("无法识别文件类型")?;
    Ok((
        gio::AppInfo::recommended_for_type(&content_type),
        gio::AppInfo::default_for_type(&content_type, false),
    ))
}

pub fn candidates(path: &Path) -> Result<Vec<Candidate>, String> {
    let (apps, default) = handlers(path)?;
    Ok(apps
        .into_iter()
        .filter_map(|app| {
            let id = app.id()?.to_string();
            // Flatpak apps may all launch through /usr/bin/flatpak. Deduplicate
            // their desktop entries, not the shared launcher executable.
            let location = app
                .clone()
                .downcast::<gio::DesktopAppInfo>()
                .ok()
                .and_then(|desktop| desktop.filename())
                .unwrap_or_else(|| PathBuf::from(&id));
            Some(Candidate {
                app: Application {
                    id,
                    name: app.display_name().to_string(),
                    is_default: default.as_ref().is_some_and(|default| app.equal(default)),
                    icon: None,
                },
                location,
                visible: app.should_show(),
                recommended: true,
            })
        })
        .collect())
}

// GTK's icon theme belongs to the UI thread. The command runs on a worker;
// schedule one batch on the existing desktop loop, not a second GTK instance.
pub fn load_icons(_: &Path, candidates: &mut [Candidate]) {
    fn resolve(candidates: &mut [Candidate]) {
        use gtk::prelude::*;
        let Some(theme) = gtk::IconTheme::default() else {
            return;
        };
        for candidate in candidates {
            candidate.app.icon = cached_icon(candidate, || {
                let app = gio::DesktopAppInfo::new(&candidate.app.id)?;
                let icon = app.icon()?;
                let image = theme
                    .lookup_by_gicon(&icon, 48, gtk::IconLookupFlags::FORCE_SIZE)?
                    .load_icon()
                    .ok()?;
                image.save_to_bufferv("png", &[]).ok()
            });
        }
    }
    if gtk::is_initialized_main_thread() {
        resolve(candidates);
    } else if gtk::is_initialized() {
        let mut batch = candidates.to_vec();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        gio::glib::idle_add_once(move || {
            resolve(&mut batch);
            let _ = send.send(batch);
        });
        if let Ok(batch) = receive.recv_timeout(std::time::Duration::from_secs(3)) {
            candidates.clone_from_slice(&batch);
        }
    }
}

pub fn launch(path: &Path, candidate: &Candidate) -> Result<(), String> {
    let (apps, _) = handlers(path)?;
    let app = apps
        .into_iter()
        .find(|app| app.id().as_deref() == Some(candidate.app.id.as_str()))
        .ok_or("此应用已不可用")?;
    app.launch(&[gio::File::for_path(path)], gio::AppLaunchContext::NONE)
        .map_err(|_| "应用无法打开此文件".into())
}
