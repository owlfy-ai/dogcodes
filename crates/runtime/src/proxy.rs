//! Proxy environment inherited by coding-agent child processes.

use std::{collections::BTreeMap, ffi::OsString};

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
use std::collections::BTreeSet;

#[cfg(target_os = "macos")]
use core_foundation::{
    base::TCFType,
    propertylist::{create_data, kCFPropertyListXMLFormat_v1_0},
};
#[cfg(target_os = "macos")]
use plist::{Dictionary as PlistDictionary, Value as PlistValue};
use proxy_watch::{ProxyEndpoint, ProxyScheme, Url};
#[cfg(any(target_os = "windows", target_os = "linux", test))]
use proxy_watch::{ProxyMode, Scheme};
#[cfg(target_os = "macos")]
use system_configuration::dynamic_store::SCDynamicStoreBuilder;
use tokio::process::Command;

const PROXY_PAIRS: [(&str, &str); 4] = [
    ("HTTP_PROXY", "http_proxy"),
    ("HTTPS_PROXY", "https_proxy"),
    ("ALL_PROXY", "all_proxy"),
    ("NO_PROXY", "no_proxy"),
];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ProxySettings {
    http: Option<String>,
    https: Option<String>,
    all: Option<String>,
    no_proxy: Option<String>,
}

/// Applies inherited proxy variables, falling back to the operating system's
/// effective proxy settings. Both upper- and lower-case names are populated
/// because coding-agent CLIs use different HTTP stacks.
pub fn apply_proxy_environment(command: &mut Command) {
    for (name, value) in resolved_proxy_environment() {
        command.env(name, value);
    }
}

fn resolved_proxy_environment() -> BTreeMap<String, OsString> {
    let inherited: BTreeMap<String, OsString> = PROXY_PAIRS
        .into_iter()
        .flat_map(|(upper, lower)| [upper, lower])
        .filter_map(|name| std::env::var_os(name).map(|value| (name.to_owned(), value)))
        .collect();
    let needs_system = PROXY_PAIRS.into_iter().any(|(upper, lower)| {
        inherited.get(lower).is_none_or(|value| value.is_empty())
            && inherited.get(upper).is_none_or(|value| value.is_empty())
    });
    let system = if needs_system {
        system_proxy_settings()
    } else {
        ProxySettings::default()
    };
    merge_proxy_environment(&inherited, system)
}

fn merge_proxy_environment(
    inherited: &BTreeMap<String, OsString>,
    system: ProxySettings,
) -> BTreeMap<String, OsString> {
    let fallbacks = [system.http, system.https, system.all, system.no_proxy];
    let mut result = BTreeMap::new();
    for ((upper, lower), fallback) in PROXY_PAIRS.into_iter().zip(fallbacks) {
        let value = inherited
            .get(lower)
            .filter(|value| !value.is_empty())
            .or_else(|| inherited.get(upper).filter(|value| !value.is_empty()))
            .cloned()
            .or_else(|| fallback.map(OsString::from));
        if let Some(value) = value {
            result.insert(upper.to_owned(), value.clone());
            result.insert(lower.to_owned(), value);
        }
    }
    result
}

#[cfg(target_os = "macos")]
fn system_proxy_settings() -> ProxySettings {
    macos_system_proxy_settings().unwrap_or_default()
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn system_proxy_settings() -> ProxySettings {
    let Ok(config) = proxy_watch::read() else {
        return ProxySettings::default();
    };
    proxy_settings_from_mode(&config.effective)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn system_proxy_settings() -> ProxySettings {
    ProxySettings::default()
}

#[cfg(target_os = "macos")]
fn macos_system_proxy_settings() -> Option<ProxySettings> {
    let store = SCDynamicStoreBuilder::new("dogcodes proxy reader").build()?;
    let proxies = store.get_proxies()?;
    let data = create_data(proxies.as_CFTypeRef(), kCFPropertyListXMLFormat_v1_0).ok()?;
    let value = PlistValue::from_reader_xml(data.bytes()).ok()?;
    value
        .as_dictionary()
        .map(macos_proxy_settings_from_dictionary)
}

#[cfg(target_os = "macos")]
fn macos_proxy_settings_from_dictionary(dictionary: &PlistDictionary) -> ProxySettings {
    let http = macos_proxy_endpoint(dictionary, "HTTP", ProxyScheme::Http, 80);
    let https = macos_proxy_endpoint(dictionary, "HTTPS", ProxyScheme::Http, 80);
    let all = macos_proxy_endpoint(dictionary, "SOCKS", ProxyScheme::Socks5, 1080);
    let no_proxy = (http.is_some() || https.is_some() || all.is_some())
        .then(|| macos_no_proxy_value(dictionary))
        .flatten();
    ProxySettings {
        http,
        https,
        all,
        no_proxy,
    }
}

#[cfg(target_os = "macos")]
fn macos_proxy_endpoint(
    dictionary: &PlistDictionary,
    prefix: &str,
    scheme: ProxyScheme,
    default_port: u16,
) -> Option<String> {
    if macos_integer(dictionary, &format!("{prefix}Enable")) != Some(1) {
        return None;
    }
    let host = dictionary.get(&format!("{prefix}Proxy"))?.as_string()?;
    let port = macos_integer(dictionary, &format!("{prefix}Port"))
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)
        .unwrap_or(default_port);
    let mut endpoint = ProxyEndpoint::parse(host, port).ok()?;
    endpoint.port = port;
    endpoint.scheme_hint = Some(scheme);
    proxy_endpoint_url(&endpoint, scheme)
}

#[cfg(target_os = "macos")]
fn macos_integer(dictionary: &PlistDictionary, key: &str) -> Option<i64> {
    let value = dictionary.get(key)?;
    value
        .as_signed_integer()
        .or_else(|| {
            value
                .as_unsigned_integer()
                .and_then(|value| value.try_into().ok())
        })
        .or_else(|| value.as_boolean().map(i64::from))
}

#[cfg(target_os = "macos")]
fn macos_no_proxy_value(dictionary: &PlistDictionary) -> Option<String> {
    let mut entries: BTreeSet<String> = dictionary
        .get("ExceptionsList")
        .and_then(PlistValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(PlistValue::as_string)
        .map(str::to_owned)
        .collect();
    for local in ["localhost", "127.0.0.1", "::1"] {
        entries.insert(local.to_owned());
    }
    (!entries.is_empty()).then(|| entries.into_iter().collect::<Vec<_>>().join(","))
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn proxy_settings_from_mode(mode: &ProxyMode) -> ProxySettings {
    let http = mode
        .endpoint_for(Scheme::Http)
        .and_then(|endpoint| proxy_endpoint_url(endpoint, ProxyScheme::Http));
    let https = mode
        .endpoint_for(Scheme::Https)
        .and_then(|endpoint| proxy_endpoint_url(endpoint, ProxyScheme::Http));
    let all = mode
        .endpoint_for(Scheme::All)
        .and_then(|endpoint| proxy_endpoint_url(endpoint, ProxyScheme::Http))
        .or_else(|| {
            mode.endpoint_for(Scheme::Socks)
                .and_then(|endpoint| proxy_endpoint_url(endpoint, ProxyScheme::Socks5))
        });
    let no_proxy = (http.is_some() || https.is_some() || all.is_some())
        .then(|| no_proxy_value(mode))
        .flatten();
    ProxySettings {
        http,
        https,
        all,
        no_proxy,
    }
}

fn proxy_endpoint_url(endpoint: &ProxyEndpoint, default_scheme: ProxyScheme) -> Option<String> {
    let scheme = endpoint.scheme_hint.unwrap_or(default_scheme).as_str();
    let mut url = Url::parse(&format!("{scheme}://{}", endpoint.authority())).ok()?;
    if let Some(auth) = &endpoint.auth {
        url.set_username(auth.username()).ok()?;
        url.set_password(auth.password()).ok()?;
    }
    let mut value = url.to_string();
    if value.ends_with('/') {
        value.pop();
    }
    Some(value)
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn no_proxy_value(mode: &ProxyMode) -> Option<String> {
    let bypass = mode.bypass()?;
    let mut entries = BTreeSet::new();
    if !bypass.reversed_exceptions {
        entries.extend(
            bypass
                .patterns
                .iter()
                .map(ToString::to_string)
                .filter(|value| value != "<local>" && value != "<-loopback>"),
        );
    }
    if bypass.bypass_loopback() {
        for local in ["localhost", "127.0.0.1", "::1"] {
            entries.insert(local.to_owned());
        }
    }
    (!entries.is_empty()).then(|| entries.into_iter().collect::<Vec<_>>().join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_static_system_proxies_and_bypass_rules() {
        let mut per_scheme = std::collections::HashMap::new();
        per_scheme.insert(
            Scheme::Http,
            proxy_watch::ProxyEntry::Use(
                ProxyEndpoint::parse("127.0.0.1:7890", 80).expect("valid HTTP proxy"),
            ),
        );
        per_scheme.insert(
            Scheme::Https,
            proxy_watch::ProxyEntry::Use(
                ProxyEndpoint::parse("127.0.0.1:7890", 80).expect("valid HTTPS proxy"),
            ),
        );
        per_scheme.insert(
            Scheme::Socks,
            proxy_watch::ProxyEntry::Use(
                ProxyEndpoint::parse("socks5://[::1]:7891", 1080).expect("valid SOCKS proxy"),
            ),
        );
        let bypass = proxy_watch::parse::no_proxy("*.local,169.254.0.0/16");
        let settings = proxy_settings_from_mode(&ProxyMode::manual(per_scheme, bypass));
        assert_eq!(settings.http.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(settings.https.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(settings.all.as_deref(), Some("socks5://[::1]:7891"));
        assert_eq!(
            settings.no_proxy.as_deref(),
            Some(".local,127.0.0.1,169.254.0.0/16,::1,localhost")
        );
    }

    #[test]
    fn direct_or_automatic_system_modes_do_not_invent_static_environment_variables() {
        let settings = proxy_settings_from_mode(&ProxyMode::Direct);
        assert_eq!(settings, ProxySettings::default());
        assert_eq!(
            proxy_settings_from_mode(&ProxyMode::WpadAutoDetect),
            ProxySettings::default()
        );
    }

    #[test]
    fn inherited_lowercase_proxy_wins_and_is_mirrored() {
        let inherited = BTreeMap::from([
            (
                "http_proxy".to_owned(),
                OsString::from("http://127.0.0.1:7890"),
            ),
            (
                "HTTP_PROXY".to_owned(),
                OsString::from("http://ignored.invalid:8080"),
            ),
        ]);
        let merged = merge_proxy_environment(
            &inherited,
            ProxySettings {
                http: Some("http://system.invalid:9000".into()),
                https: Some("http://system.invalid:9000".into()),
                ..ProxySettings::default()
            },
        );
        assert_eq!(merged["HTTP_PROXY"], "http://127.0.0.1:7890");
        assert_eq!(merged["http_proxy"], "http://127.0.0.1:7890");
        assert_eq!(merged["HTTPS_PROXY"], "http://system.invalid:9000");
        assert_eq!(merged["https_proxy"], "http://system.invalid:9000");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn converts_macos_system_configuration_dictionary() {
        let mut dictionary = PlistDictionary::new();
        dictionary.insert("HTTPEnable".into(), PlistValue::Integer(1.into()));
        dictionary.insert("HTTPProxy".into(), PlistValue::String("127.0.0.1".into()));
        dictionary.insert("HTTPPort".into(), PlistValue::Integer(7890.into()));
        dictionary.insert("HTTPSEnable".into(), PlistValue::Integer(1.into()));
        dictionary.insert("HTTPSProxy".into(), PlistValue::String("127.0.0.1".into()));
        dictionary.insert("HTTPSPort".into(), PlistValue::Integer(7890.into()));
        dictionary.insert("SOCKSEnable".into(), PlistValue::Integer(1.into()));
        dictionary.insert("SOCKSProxy".into(), PlistValue::String("127.0.0.1".into()));
        dictionary.insert("SOCKSPort".into(), PlistValue::Integer(7891.into()));
        dictionary.insert(
            "ExceptionsList".into(),
            PlistValue::Array(vec![PlistValue::String("*.internal".into())]),
        );

        let settings = macos_proxy_settings_from_dictionary(&dictionary);
        assert_eq!(settings.http.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(settings.https.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(settings.all.as_deref(), Some("socks5://127.0.0.1:7891"));
        assert_eq!(
            settings.no_proxy.as_deref(),
            Some("*.internal,127.0.0.1,::1,localhost")
        );
    }

    #[test]
    #[ignore = "requires a workstation with a known system proxy"]
    fn probes_native_system_proxy_without_external_commands() {
        let expected = std::env::var("DOGCODES_EXPECTED_SYSTEM_HTTP_PROXY")
            .expect("set DOGCODES_EXPECTED_SYSTEM_HTTP_PROXY");
        assert_eq!(
            system_proxy_settings().http.as_deref(),
            Some(expected.as_str())
        );
    }
}
