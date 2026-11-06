//! The bundled CLI and its desktop host have independent version numbers.
use super::{AgentUpdate, get, newer};
use serde::Deserialize;
use std::path::Path;

// Verified against codexSparkleFeedUrl in the production desktop's package metadata.
const FEED: &str = "https://persistent.oaistatic.com/codex-app-prod/appcast.xml";
const BUNDLE_ID: &str = "com.openai.codex";

#[derive(Deserialize)]
struct Appcast {
    channel: Channel,
}
#[derive(Deserialize)]
struct Channel {
    #[serde(rename = "item", default)]
    items: Vec<Release>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Release {
    short_version_string: Option<String>,
    hardware_requirements: Option<String>,
    minimum_system_version: Option<String>,
    channel: Option<String>,
    enclosure: Option<Enclosure>,
}
#[derive(Deserialize)]
struct Enclosure {
    #[serde(rename = "@url")]
    url: String,
}

fn plist_string(info: &plist::Value, key: &str) -> Option<String> {
    info.as_dictionary()?
        .get(key)?
        .as_string()
        .map(str::to_owned)
}

pub(super) fn desktop_app(executable: &Path) -> Option<&Path> {
    // New desktop layouts nest CodexCLI.app inside the owning desktop bundle.
    // Identify the desktop by its bundle ID, not the nearest .app or its name.
    executable.ancestors().find(|app| {
        app.extension().is_some_and(|v| v == "app")
            && plist::Value::from_file(app.join("Contents/Info.plist"))
                .ok()
                .and_then(|info| plist_string(&info, "CFBundleIdentifier"))
                .as_deref()
                == Some(BUNDLE_ID)
    })
}

fn system_version(value: &str) -> Option<Vec<u64>> {
    value.trim().split('.').map(|p| p.parse().ok()).collect()
}

fn latest_release(xml: &str, arch: &str, os: &str) -> Result<String, String> {
    let feed: Appcast = quick_xml::de::from_str(xml).map_err(|_| "Codex 更新源格式无效")?;
    let os = system_version(os).ok_or("无法识别 macOS 版本")?;
    feed.channel
        .items
        .into_iter()
        .filter(|release| {
            release
                .channel
                .as_deref()
                .is_none_or(|c| c.is_empty() || c == "stable")
                && release
                    .hardware_requirements
                    .as_deref()
                    .is_none_or(|a| a.trim() == arch)
                && release
                    .minimum_system_version
                    .as_deref()
                    .is_none_or(|minimum| {
                        system_version(minimum).is_some_and(|mut minimum| {
                            let mut current = os.clone();
                            let length = current.len().max(minimum.len());
                            current.resize(length, 0);
                            minimum.resize(length, 0);
                            current >= minimum
                        })
                    })
                && release.enclosure.as_ref().is_some_and(|e| {
                    e.url
                        .starts_with("https://persistent.oaistatic.com/codex-app-prod/")
                })
        })
        .filter_map(|release| {
            release
                .short_version_string
                .and_then(|v| semver::Version::parse(v.trim()).ok())
        })
        .max()
        .map(|v| v.to_string())
        .ok_or("Codex 更新源未提供适用于当前 Mac 的正式版本".into())
}

pub(super) async fn inspect(app: &Path, mut out: AgentUpdate) -> Result<AgentUpdate, String> {
    let info = plist::Value::from_file(app.join("Contents/Info.plist"))
        .map_err(|_| "无法读取 Codex 桌面应用版本")?;
    if plist_string(&info, "CFBundleIdentifier").as_deref() != Some(BUNDLE_ID) {
        return Err("此 Codex 由其他宿主应用提供，请在原应用内检查更新".into());
    }
    let current =
        plist_string(&info, "CFBundleShortVersionString").ok_or("Codex 桌面应用版本未知")?;
    let system = plist::Value::from_file("/System/Library/CoreServices/SystemVersion.plist")
        .map_err(|_| "无法读取 macOS 版本")?;
    let os = plist_string(&system, "ProductVersion").ok_or("macOS 版本未知")?;
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    let latest = latest_release(&get(FEED).await?, arch, &os)?;
    out.message = Some(if newer(&current, &latest)? {
        format!("Codex 桌面应用 {current} 可升级至 {latest}，请在 Codex 中完成升级")
    } else {
        format!("Codex 桌面应用 {current} 已是最新正式版本")
    });
    out.source = "Codex 桌面应用 · 官方更新源".into();
    out.current_version = Some(current);
    out.latest_version = Some(latest);
    // Sparkle owns installation/signature validation. Never overwrite a bundled CLI.
    out.can_upgrade = false;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(v: &str, extra: &str) -> String {
        format!(
            r#"<item><sparkle:shortVersionString>{v}</sparkle:shortVersionString>{extra}<enclosure url="https://persistent.oaistatic.com/codex-app-prod/app.zip"/></item>"#
        )
    }
    fn feed(items: &str) -> String {
        format!(
            r#"<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel>{items}</channel></rss>"#
        )
    }

    #[test]
    fn selects_latest_compatible_stable_desktop_version_not_cli_or_feed_order() {
        let xml = feed(&[
            item("26.911.61220", ""),
            item("99.1.0", "<sparkle:channel>beta</sparkle:channel>"),
            item("98.1.0", "<sparkle:hardwareRequirements>x86_64</sparkle:hardwareRequirements>"),
            item("97.1.0", "<sparkle:minimumSystemVersion>99.0</sparkle:minimumSystemVersion>"),
            item("26.915.31945", "<sparkle:hardwareRequirements>arm64</sparkle:hardwareRequirements><sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>"),
            item("26.915.31029", ""),
        ].concat());
        assert_eq!(latest_release(&xml, "arm64", "13").unwrap(), "26.915.31945");
        assert!(
            newer(
                "26.911.61220",
                &latest_release(&xml, "arm64", "26.0").unwrap()
            )
            .unwrap()
        );
    }

    #[test]
    fn fails_explicitly_for_invalid_or_incompatible_feeds() {
        for xml in [
            "not xml".into(),
            feed(""),
            feed(&item("unknown", "")),
            feed(&item(
                "26.1.0",
                "<sparkle:hardwareRequirements>x86_64</sparkle:hardwareRequirements>",
            )),
        ] {
            assert!(latest_release(&xml, "arm64", "26.0").is_err());
        }
    }

    #[tokio::test]
    #[ignore = "read-only check of installed Codex desktop and official feed"]
    async fn live_codex_desktop_check() {
        let mut checked = 0;
        for app in ["/Applications/ChatGPT.app", "/Applications/Codex.app"] {
            for relative in [
                "Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
                "Contents/Resources/codex-cli/bin/codex",
                "Contents/Resources/codex",
            ] {
                let executable = Path::new(app).join(relative);
                if !executable.is_file() {
                    continue;
                }
                let actual = super::super::command(super::super::spec(
                    &executable,
                    vec!["--version".into()],
                    false,
                ))
                .await
                .unwrap();
                let version = super::super::version(&actual).unwrap().to_string();
                let i = super::super::tests::installation_for("codex", &executable, &version);
                let result = super::super::AgentUpdates::default().check(&i, true).await;
                println!("{relative}: {}", serde_json::to_string(&result).unwrap());
                assert_eq!(result.status, "managed");
                assert!(!result.can_upgrade);
                assert_ne!(result.current_version, i.version);
                assert_eq!(result.desktop_version, result.current_version);
                assert!(result.latest_version.is_some());
                checked += 1;
            }
        }
        assert!(checked > 0, "no installed Codex desktop CLI found");
    }
}
