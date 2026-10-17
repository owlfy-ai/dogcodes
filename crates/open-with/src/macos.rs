use super::*;
use objc2::AllocAnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
use objc2_foundation::{NSDictionary, NSFileManager, NSPoint, NSRect, NSSize, NSString, NSURL};

pub fn candidates(path: &Path) -> Result<Vec<Candidate>, String> {
    let workspace = NSWorkspace::sharedWorkspace();
    let url = NSURL::fileURLWithPath(&NSString::from_str(
        path.to_str().ok_or("文件路径编码不可用")?,
    ));
    let default_path = workspace
        .URLForApplicationToOpenURL(&url)
        .and_then(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()));
    let bundle_info = |path: &Path| plist::Value::from_file(path.join("Contents/Info.plist")).ok();
    let default_id = default_path
        .as_deref()
        .and_then(bundle_info)
        .and_then(|value| {
            value
                .as_dictionary()?
                .get("CFBundleIdentifier")?
                .as_string()
                .map(str::to_owned)
        });
    // macOS provides file-associated apps, not a separate 'recommended' flag.
    // Use this system list only; do not add generic editors or scan Applications.
    let mut result = Vec::new();
    for url in workspace.URLsForApplicationsToOpenURL(&url).iter() {
        let Some(path) = url.path() else {
            continue;
        };
        let Ok(location) = PathBuf::from(path.to_string()).canonicalize() else {
            continue;
        };
        let Some(value) = bundle_info(&location) else {
            continue;
        };
        let Some(info) = value.as_dictionary() else {
            continue;
        };
        let Some(id) = info
            .get("CFBundleIdentifier")
            .and_then(plist::Value::as_string)
        else {
            continue;
        };
        // Match Finder's localized application name rather than internal names
        // such as "BaiduNetdisk_mac" from the unlocalized Info.plist.
        let name = NSFileManager::defaultManager()
            .displayNameAtPath(&path)
            .to_string();
        let name = name.strip_suffix(".app").unwrap_or(&name);
        let hidden = ["LSUIElement", "LSBackgroundOnly"].iter().any(|key| {
            info.get(key).is_some_and(|value| {
                value.as_boolean() == Some(true)
                    || value.as_signed_integer() == Some(1)
                    || value.as_string().is_some_and(|value| {
                        matches!(value.to_lowercase().as_str(), "true" | "yes" | "1")
                    })
            })
        });
        result.push(Candidate {
            app: Application {
                id: id.into(),
                name: name.into(),
                is_default: default_id.as_deref() == Some(id),
                icon: None,
            },
            location,
            visible: !hidden,
            recommended: true,
        });
    }
    Ok(result)
}

pub fn load_icons(_: &Path, candidates: &mut [Candidate]) {
    objc2::rc::autoreleasepool(|_| {
        for candidate in candidates {
            candidate.app.icon = cached_icon(candidate, || {
                let image = NSWorkspace::sharedWorkspace()
                    .iconForFile(&NSString::from_str(candidate.location.to_str()?));
                let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(48.0, 48.0));
                // SAFETY: rect is valid for this call; no context or typed hints
                // are passed. All image objects remain local to this thread.
                let cg =
                    unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
                let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
                // SAFETY: An empty properties dictionary contains no mismatched values.
                let png = unsafe {
                    bitmap.representationUsingType_properties(
                        NSBitmapImageFileType::PNG,
                        &NSDictionary::new(),
                    )
                }?;
                Some(png.to_vec())
            });
        }
    });
}

pub fn launch(path: &Path, candidate: &Candidate) -> Result<(), String> {
    let status = std::process::Command::new("/usr/bin/open")
        .arg("-a")
        .arg(&candidate.location)
        .arg(path)
        .status()
        .map_err(|_| "无法启动应用")?;
    if status.success() {
        Ok(())
    } else {
        Err("应用无法打开此文件".into())
    }
}
