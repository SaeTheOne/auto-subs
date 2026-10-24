//! Which translation service the text post-pass talks to, read from a
//! user-editable `config.json` that sits in the install directory.
//!
//! The desktop app calls [`set_config_path`] with `<install dir>/config.json`
//! during startup; headless/CLI runs and the `translate` example fall back to
//! `AUTOSUBS_CONFIG` and then to `config.json` next to the executable.
//!
//! The file is re-read whenever its size or modification time changes, so the
//! user can switch providers (or paste a new Microsoft key) without restarting
//! the app: the next translation batch picks the new values up.
//!
//! Two shapes are accepted, so the file can stay focused on translation today
//! and grow other sections later:
//!
//! ```json
//! { "translation": { "provider": "microsoft", "microsoft": { "key": "…" } } }
//! ```
//!
//! or the translation block itself at the top level:
//!
//! ```json
//! { "provider": "microsoft", "microsoft": { "key": "…" } }
//! ```
//!
//! Unknown keys are ignored on purpose: an older build must not choke on a
//! config written for a newer one.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

pub const DEFAULT_GOOGLE_ENDPOINT: &str = "https://clients5.google.com/translate_a/t";
/// Endpoint older builds wrote into `config.json`. `translate_a/single` (the
/// `client=gtx` frontend) is rate limited much more aggressively and returns
/// 429 to whole networks, so an unmodified file is migrated to the default
/// above instead of being honoured.
pub const LEGACY_GOOGLE_ENDPOINT: &str = "https://translate.googleapis.com/translate_a/single";
pub const DEFAULT_MICROSOFT_ENDPOINT: &str = "https://api.cognitive.microsofttranslator.com";

/// Environment override, mainly for headless runs and tests.
pub const CONFIG_PATH_ENV: &str = "AUTOSUBS_CONFIG";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Google,
    Microsoft,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Google => "google",
            Provider::Microsoft => "microsoft",
        }
    }

    fn parse(raw: &str) -> Option<Provider> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "google" | "google-translate" | "googletranslate" => Some(Provider::Google),
            "microsoft" | "azure" | "bing" | "microsoft-translator" => Some(Provider::Microsoft),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MicrosoftSettings {
    pub endpoint: String,
    pub key: String,
    pub region: Option<String>,
}

impl Default for MicrosoftSettings {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_MICROSOFT_ENDPOINT.to_string(),
            key: String::new(),
            region: None,
        }
    }
}

impl MicrosoftSettings {
    /// A Microsoft request needs a subscription key; everything else has a
    /// sensible default.
    pub fn is_configured(&self) -> bool {
        !self.key.trim().is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub provider: Provider,
    pub fallback: Option<Provider>,
    pub concurrency: usize,
    pub google_endpoint: String,
    pub microsoft: MicrosoftSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // Google keeps working with no configuration at all, so it stays
            // the default for a fresh install.
            provider: Provider::Google,
            fallback: None,
            concurrency: 2,
            google_endpoint: DEFAULT_GOOGLE_ENDPOINT.to_string(),
            microsoft: MicrosoftSettings::default(),
        }
    }
}

impl Settings {
    /// The provider to try first and the one to fall back to, after skipping
    /// anything that cannot work with the credentials on hand.
    pub fn plan(&self) -> (Provider, Option<Provider>) {
        let usable = |p: Provider| match p {
            Provider::Google => true,
            Provider::Microsoft => self.microsoft.is_configured(),
        };

        if usable(self.provider) {
            let fallback = self
                .fallback
                .filter(|p| *p != self.provider && usable(*p));
            return (self.provider, fallback);
        }

        tracing::warn!(
            provider = self.provider.as_str(),
            "translation provider is selected but has no credentials configured; using google"
        );
        (Provider::Google, None)
    }
}

/// Snapshot for the UI / logs: no secrets, just enough to tell the user which
/// service is in play and where the file lives.
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub provider: String,
    pub fallback: Option<String>,
    pub microsoft_configured: bool,
    pub microsoft_region: Option<String>,
    pub config_path: String,
    pub config_present: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawMicrosoft {
    endpoint: Option<String>,
    key: Option<String>,
    region: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawGoogle {
    endpoint: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawTranslation {
    provider: Option<String>,
    fallback: Option<String>,
    concurrency: Option<usize>,
    google: RawGoogle,
    microsoft: RawMicrosoft,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawFile {
    translation: Option<RawTranslation>,
    // Root-level copy of the same keys, used when there is no `translation`
    // section.
    provider: Option<String>,
    fallback: Option<String>,
    concurrency: Option<usize>,
    google: RawGoogle,
    microsoft: RawMicrosoft,
}

impl RawFile {
    fn translation(&self) -> RawTranslation {
        match &self.translation {
            Some(t) => RawTranslation {
                provider: t.provider.clone(),
                fallback: t.fallback.clone(),
                concurrency: t.concurrency,
                google: RawGoogle {
                    endpoint: t.google.endpoint.clone(),
                },
                microsoft: RawMicrosoft {
                    endpoint: t.microsoft.endpoint.clone(),
                    key: t.microsoft.key.clone(),
                    region: t.microsoft.region.clone(),
                },
            },
            None => RawTranslation {
                provider: self.provider.clone(),
                fallback: self.fallback.clone(),
                concurrency: self.concurrency,
                google: RawGoogle {
                    endpoint: self.google.endpoint.clone(),
                },
                microsoft: RawMicrosoft {
                    endpoint: self.microsoft.endpoint.clone(),
                    key: self.microsoft.key.clone(),
                    region: self.microsoft.region.clone(),
                },
            },
        }
    }
}

struct Loaded {
    settings: Settings,
    path: Option<PathBuf>,
    config_present: bool,
    error: Option<String>,
}

struct Cache {
    /// Path handed to us by the app; `None` means "derive it".
    explicit_path: Option<PathBuf>,
    /// Path the cached settings were read from.
    loaded_path: Option<PathBuf>,
    stamp: Option<(u64, Option<SystemTime>)>,
    loaded: Loaded,
}

impl Cache {
    fn new() -> Self {
        Self {
            explicit_path: None,
            loaded_path: None,
            stamp: None,
            loaded: Loaded {
                settings: Settings::default(),
                path: None,
                config_present: false,
                error: None,
            },
        }
    }
}

static CACHE: Lazy<Mutex<Cache>> = Lazy::new(|| Mutex::new(Cache::new()));

/// Point the loader at a specific file (the app calls this with the install
/// directory's `config.json`).
pub fn set_config_path(path: impl Into<PathBuf>) {
    let mut cache = CACHE.lock().unwrap();
    cache.explicit_path = Some(path.into());
    // Force a re-read on the next access.
    cache.stamp = None;
}

/// The path the configuration is read from, after applying the app override,
/// the environment variable and the executable-relative default.
pub fn config_path() -> PathBuf {
    let cache = CACHE.lock().unwrap();
    resolve_path(&cache)
}

fn resolve_path(cache: &Cache) -> PathBuf {
    if let Some(path) = &cache.explicit_path {
        return path.clone();
    }
    if let Some(path) = std::env::var_os(CONFIG_PATH_ENV) {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        return dir.join("config.json");
    }
    PathBuf::from("config.json")
}

fn read_stamp(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some((meta.len(), meta.modified().ok()))
}

/// Re-read the file when it changed. Cheap enough to call per batch: the
/// common case is one `stat`.
fn refresh(cache: &mut Cache) {
    let path = resolve_path(cache);
    let stamp = read_stamp(&path);

    if cache.loaded_path.as_deref() == Some(path.as_path()) && cache.stamp == stamp {
        return;
    }

    let Some(stamp_value) = stamp else {
        // Missing file: defaults, no complaint (the app writes one on first run).
        cache.loaded = Loaded {
            settings: Settings::default(),
            path: Some(path),
            config_present: false,
            error: None,
        };
        cache.loaded_path = cache.loaded.path.clone();
        cache.stamp = None;
        return;
    };

    match parse_file(&path) {
        Ok(settings) => {
            tracing::info!(
                path = %path.display(),
                provider = settings.provider.as_str(),
                fallback = settings.fallback.map(|p| p.as_str()).unwrap_or("none"),
                "translation configuration loaded"
            );
            cache.loaded = Loaded {
                settings,
                path: Some(path.clone()),
                config_present: true,
                error: None,
            };
        }
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "translation configuration is invalid; using google translate"
            );
            cache.loaded = Loaded {
                settings: Settings::default(),
                path: Some(path.clone()),
                config_present: true,
                error: Some(error),
            };
        }
    }

    cache.loaded_path = Some(path);
    cache.stamp = Some(stamp_value);
}

fn parse_file(path: &Path) -> Result<Settings, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read failed: {e}"))?;
    parse_str(&text)
}

fn parse_str(text: &str) -> Result<Settings, String> {
    let raw: RawFile = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    let translation = raw.translation();

    let provider = match translation.provider.as_deref() {
        None => Provider::Google,
        Some(raw_provider) => Provider::parse(raw_provider).ok_or_else(|| {
            format!(
                "unknown translation.provider {:?}; expected \"google\" or \"microsoft\"",
                raw_provider
            )
        })?,
    };

    let fallback = translation
        .fallback
        .as_deref()
        .map(|raw_fallback| {
            Provider::parse(raw_fallback).ok_or_else(|| {
                format!(
                    "unknown translation.fallback {:?}; expected \"google\", \"microsoft\" or null",
                    raw_fallback
                )
            })
        })
        .transpose()?;

    let concurrency = translation.concurrency.unwrap_or(2).clamp(1, 16);

    let google_endpoint = translation
        .google
        .endpoint
        .filter(|e| !e.trim().is_empty())
        .map(|endpoint| {
            if endpoint.trim() == LEGACY_GOOGLE_ENDPOINT {
                // Keep old config files working: the endpoint that used to be
                // the default is the one that now answers 429.
                DEFAULT_GOOGLE_ENDPOINT.to_string()
            } else {
                endpoint
            }
        })
        .unwrap_or_else(|| DEFAULT_GOOGLE_ENDPOINT.to_string());

    let microsoft = MicrosoftSettings {
        endpoint: translation
            .microsoft
            .endpoint
            .filter(|e| !e.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MICROSOFT_ENDPOINT.to_string()),
        key: translation.microsoft.key.unwrap_or_default(),
        region: translation
            .microsoft
            .region
            .filter(|r| !r.trim().is_empty()),
    };

    Ok(Settings {
        provider,
        fallback,
        concurrency,
        google_endpoint,
        microsoft,
    })
}

/// Current settings, re-reading the file if it changed.
pub fn settings() -> Settings {
    let mut cache = CACHE.lock().unwrap();
    refresh(&mut cache);
    cache.loaded.settings.clone()
}

/// Force a re-read (used after the user edits the file in place without a
/// size/mtime change, and by the UI's "reload" action).
pub fn reload() -> Result<Status, String> {
    {
        let mut cache = CACHE.lock().unwrap();
        cache.stamp = None;
        refresh(&mut cache);
    }
    status()
}

/// Current provider summary for the UI and logs.
pub fn status() -> Result<Status, String> {
    let mut cache = CACHE.lock().unwrap();
    refresh(&mut cache);
    let loaded = &cache.loaded;
    let (primary, fallback) = loaded.settings.plan();
    Ok(Status {
        provider: primary.as_str().to_string(),
        fallback: fallback.map(|p| p.as_str().to_string()),
        microsoft_configured: loaded.settings.microsoft.is_configured(),
        microsoft_region: loaded.settings.microsoft.region.clone(),
        config_path: loaded
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        config_present: loaded.config_present,
        error: loaded.error.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_and_root_shapes_are_equivalent() {
        let nested = parse_str(
            r#"{"translation":{"provider":"microsoft","microsoft":{"key":"k","region":"eastasia"}}}"#,
        )
        .unwrap();
        let flat = parse_str(r#"{"provider":"microsoft","microsoft":{"key":"k","region":"eastasia"}}"#)
            .unwrap();

        assert_eq!(nested.provider, Provider::Microsoft);
        assert_eq!(flat.provider, Provider::Microsoft);
        assert_eq!(nested.microsoft.key, "k");
        assert_eq!(flat.microsoft.key, "k");
        assert_eq!(flat.microsoft.region.as_deref(), Some("eastasia"));
    }

    #[test]
    fn microsoft_without_key_falls_back_to_google() {
        let settings = parse_str(r#"{"provider":"microsoft"}"#).unwrap();
        assert_eq!(settings.plan(), (Provider::Google, None));
    }

    #[test]
    fn configured_microsoft_keeps_its_fallback() {
        let settings =
            parse_str(r#"{"provider":"microsoft","fallback":"google","microsoft":{"key":"k"}}"#)
                .unwrap();
        assert_eq!(settings.plan(), (Provider::Microsoft, Some(Provider::Google)));
    }

    #[test]
    fn unknown_provider_names_are_reported() {
        let err = parse_str(r#"{"provider":"deepl"}"#).unwrap_err();
        assert!(err.contains("deepl"), "{err}");
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let settings = parse_str(
            r#"{"_readme":"hi","translation":{"provider":"google","future":{"x":1}}}"#,
        )
        .unwrap();
        assert_eq!(settings.provider, Provider::Google);
    }
}
