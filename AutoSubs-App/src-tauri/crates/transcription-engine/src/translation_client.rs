//! Text translation post-pass.
//!
//! Two services are supported, chosen by `<install dir>/config.json` (see
//! [`crate::translation_config`]):
//!
//! * `google` — the unofficial `translate_a/single` endpoint, no credentials,
//!   one request per segment. This is the default and what shipped before the
//!   provider switch existed.
//! * `microsoft` — the Azure AI Translator v3 API, authenticated with a
//!   subscription key and region, batched (the API takes an array of texts, so
//!   a whole batch is one round trip).
//!
//! Failure policy is unchanged from the Google-only version, because callers
//! depend on it:
//!
//! * a segment that keeps failing after retries keeps its original text, so a
//!   few bad rows never abort a transcription;
//! * the batch only errors when *every* non-empty segment failed for a
//!   service-side reason, which means the service is down (or, for Microsoft,
//!   the key/region is wrong) and silently emitting source text as
//!   "translated" would be worse than an error;
//! * when `translation.fallback` names another provider, a service-wide
//!   failure is retried once on it.

use std::sync::Arc;

use eyre::{eyre, Result};
use futures::stream::{self, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

use crate::translation_config::{self, MicrosoftSettings, Provider, Settings};
use crate::types::{Segment, WordTimestamp};

/// How a single segment (or batch chunk) failed.
#[derive(Debug, Clone)]
enum Failure {
    /// Transport error, 429 or 5xx: retrying later may help.
    Outage(String),
    /// Configuration problem (bad key/region/endpoint): retrying is pointless.
    Fatal(String),
    /// The service answered but refused this particular input.
    Rejected(String),
}

impl Failure {
    fn message(&self) -> &str {
        match self {
            Failure::Outage(m) | Failure::Fatal(m) | Failure::Rejected(m) => m,
        }
    }
}

/// Attempts per request before a segment/chunk is given up on.
const MAX_RETRIES: u32 = 5;
/// Microsoft's batch endpoint accepts up to 100 items; stay well below the
/// payload limits so one bad chunk cannot take a whole batch down.
const MICROSOFT_CHUNK: usize = 25;

// Normalize Whisper language codes to the codes accepted by the unofficial
// Google Translate endpoint. Applies both to source (sl) and target (tl) codes.
fn normalize_google_lang(code: &str, _is_target: bool) -> String {
    let c = code.trim();
    if c.eq_ignore_ascii_case("auto") {
        return "auto".to_string();
    }

    match c {
        // Whisper uses "jw" for Javanese; Google expects "jv"
        "jw" => "jv".to_string(),
        // Cantonese is not supported separately: map to Traditional Chinese
        "yue" => "zh-TW".to_string(),
        _ => c.to_string(),
    }
}

/// Whisper's ISO-639-1 codes are almost all valid BCP-47 tags for Microsoft,
/// but a handful need translating. `None` means "omit the parameter", which is
/// how the API is asked to auto-detect the source language.
fn normalize_microsoft_lang(code: &str) -> Option<String> {
    let c = code.trim();
    if c.is_empty() || c.eq_ignore_ascii_case("auto") {
        return None;
    }

    let lower = c.to_ascii_lowercase();
    let mapped: &str = match lower.as_str() {
        "zh" | "zh-cn" | "zh-sg" | "zh-hans" => "zh-Hans",
        "zh-tw" | "zh-hk" | "zh-mo" | "zh-hant" | "yue" => "zh-Hant",
        // Microsoft has no Javanese alias for "jw"
        "jw" => "jv",
        // Only Bokmål is supported; Norwegian variants map onto it
        "no" | "nn" | "nb" => "nb",
        // Tagalog/Filipino
        "tl" => "fil",
        // Legacy ISO codes still emitted by some models
        "iw" => "he",
        "in" => "id",
        _ => c,
    };

    Some(mapped.to_string())
}

fn backoff(attempt: u32) -> Duration {
    // 500ms, 1s, 2s, 4s, 8s, plus jitter from wall-clock nanos so concurrent
    // workers don't retry in lockstep after a 429.
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 % 400)
        .unwrap_or(0);
    Duration::from_millis((500u64 << attempt).min(8000) + jitter)
}

fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|secs| Duration::from_secs(secs.min(15)))
}

/// Translates a single string from one language to another.
pub async fn translate_text(text: &str, from: &str, to: &str) -> Result<String> {
    let mut translated = translate_batch(vec![text.to_string()], from, to).await?;
    Ok(translated.pop().unwrap_or_default())
}

/// Translate a collection of texts while preserving order.
///
/// Empty strings are preserved as empty and not sent to the service.
/// See the module docs for the per-segment failure policy.
pub async fn translate_batch(texts: Vec<String>, from: &str, to: &str) -> Result<Vec<String>> {
    // Fast path: source and target are the same (and not auto) so no translation needed.
    if from.eq_ignore_ascii_case(to) && !from.eq_ignore_ascii_case("auto") {
        return Ok(texts);
    }
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let settings = translation_config::settings();
    let (primary, fallback) = settings.plan();

    // One shared client: per-segment `Client::new()` defeats connection reuse,
    // which makes the translate endpoints throttle much sooner.
    let client = Arc::new(Client::new());

    let mut outcome = run_provider(primary, &client, &texts, from, to, &settings).await;

    if outcome.service_wide_failure() {
        if let Some(fallback) = fallback.filter(|f| *f != primary) {
            tracing::warn!(
                primary = primary.as_str(),
                fallback = fallback.as_str(),
                reason = outcome.reason.as_deref().unwrap_or("unknown"),
                "translation provider failed; retrying with the configured fallback"
            );
            outcome = run_provider(fallback, &client, &texts, from, to, &settings).await;
        }
    }

    if let Some(reason) = outcome.fatal_reason() {
        return Err(eyre!("{}", reason));
    }

    if outcome.attempted > 0 && outcome.failed == outcome.attempted && outcome.outage == outcome.attempted {
        return Err(eyre!(
            "translation failed for all {} segments in batch; service appears unreachable",
            outcome.attempted
        ));
    }

    if outcome.failed > 0 {
        tracing::warn!(
            "translation: {}/{} segments kept their original text after retries",
            outcome.failed,
            outcome.attempted
        );
    }

    Ok(outcome.texts)
}

/// Aggregate view of one provider pass.
struct Outcome {
    texts: Vec<String>,
    attempted: usize,
    failed: usize,
    outage: usize,
    fatal: Option<String>,
    /// First outage/fatal message, kept for logs.
    reason: Option<String>,
}

impl Outcome {
    fn service_wide_failure(&self) -> bool {
        self.fatal.is_some()
            || (self.attempted > 0 && self.failed == self.attempted && self.outage == self.attempted)
    }

    fn fatal_reason(&self) -> Option<String> {
        self.fatal.clone()
    }
}

async fn run_provider(
    provider: Provider,
    client: &Client,
    texts: &[String],
    from: &str,
    to: &str,
    settings: &Settings,
) -> Outcome {
    let results = match provider {
        Provider::Google => {
            google_batch(
                client,
                texts,
                from,
                to,
                &settings.google_endpoint,
                settings.concurrency.max(1),
            )
            .await
        }
        Provider::Microsoft => microsoft_batch(client, texts, from, to, &settings.microsoft).await,
    };

    summarize(texts, results)
}

fn summarize(texts: &[String], results: Vec<Result<String, Failure>>) -> Outcome {
    let mut texts_out = Vec::with_capacity(texts.len());
    let mut attempted = 0usize;
    let mut failed = 0usize;
    let mut outage = 0usize;
    let mut fatal: Option<String> = None;
    let mut reason: Option<String> = None;

    for (index, (original, result)) in texts.iter().zip(results.into_iter()).enumerate() {
        let is_empty = original.trim().is_empty();
        if !is_empty {
            attempted += 1;
        }

        match result {
            Ok(translated) => texts_out.push(translated),
            Err(failure) => {
                if !is_empty {
                    failed += 1;
                    match &failure {
                        Failure::Outage(message) => {
                            outage += 1;
                            reason.get_or_insert_with(|| message.clone());
                        }
                        Failure::Fatal(message) => {
                            fatal.get_or_insert_with(|| message.clone());
                            reason.get_or_insert_with(|| message.clone());
                        }
                        Failure::Rejected(_) => {}
                    }
                    tracing::warn!(
                        "translation failed for segment {}, keeping original text: {}",
                        index,
                        failure.message()
                    );
                }
                texts_out.push(original.clone());
            }
        }
    }

    Outcome {
        texts: texts_out,
        attempted,
        failed,
        outage,
        fatal,
        reason,
    }
}

// ─── Google ────────────────────────────────────────────────────────────────

async fn google_batch(
    client: &Client,
    texts: &[String],
    from: &str,
    to: &str,
    endpoint: &str,
    concurrency: usize,
) -> Vec<Result<String, Failure>> {
    let sl = normalize_google_lang(from, false);
    let tl = normalize_google_lang(to, true);

    let mut slots: Vec<Option<Result<String, Failure>>> = (0..texts.len()).map(|_| None).collect();

    let mut stream = stream::iter(texts.iter().cloned().enumerate())
        .map(|(index, text)| {
            let client = client.clone();
            let sl = sl.clone();
            let tl = tl.clone();
            let endpoint = endpoint.to_string();
            async move {
                if text.trim().is_empty() {
                    (index, Ok(String::new()))
                } else {
                    let result = google_one(&client, &endpoint, &text, &sl, &tl).await;
                    (index, result)
                }
            }
        })
        .buffer_unordered(concurrency.max(1));

    while let Some((index, result)) = stream.next().await {
        slots[index] = Some(result);
    }

    slots
        .into_iter()
        .map(|slot| slot.unwrap_or_else(|| Ok(String::new())))
        .collect()
}

async fn google_one(
    client: &Client,
    endpoint: &str,
    text: &str,
    sl: &str,
    tl: &str,
) -> Result<String, Failure> {
    // `translate_a/t` is the response used by the Chrome side-panel frontend
    // (`dict-chrome-ex`); it answers where `translate_a/single` (`client=gtx`)
    // returns 429 for whole networks. Both are the same translation backend,
    // only the query and the payload shape differ.
    let chrome_ex = endpoint.contains("/translate_a/t");

    let mut attempt = 0u32;
    loop {
        let request = client
            .get(endpoint)
            .header("sec-ch-ua", "\"Google Chrome\";v=\"129\", \"Not=A?Brand\";v=\"8\", \"Chromium\";v=\"129\"")
            .header("x-browser-year", "2024")
            .header("x-browser-channel", "stable")
            .header("sec-ch-ua-arch", "\"x86\"")
            .header("sec-ch-ua-bitness", "\"64\"")
            .header("sec-ch-ua-form-factors", "\"Desktop\"")
            .header("x-browser-copyright", "Copyright 2024 Google LLC. All rights reserved.");

        let request = if chrome_ex {
            request.query(&[("client", "dict-chrome-ex"), ("sl", sl), ("tl", tl), ("q", text)])
        } else {
            request.query(&[("client", "gtx"), ("sl", sl), ("tl", tl), ("dt", "t"), ("q", text)])
        };

        let resp_result = request.send().await;

        match resp_result {
            Ok(resp) => {
                if resp.status().is_success() {
                    let body = match resp.text().await {
                        Ok(body) => body,
                        Err(e) => return Err(Failure::Rejected(e.to_string())),
                    };
                    let translated = serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|json| parse_google_payload(&json))
                        .unwrap_or_default();
                    return Ok(translated);
                }

                if resp.status().as_u16() == 429 || resp.status().is_server_error() {
                    if attempt >= MAX_RETRIES {
                        return Err(Failure::Outage(format!(
                            "google translate HTTP {} after retries",
                            resp.status()
                        )));
                    }
                    sleep(backoff(attempt)).await;
                    attempt += 1;
                    continue;
                }

                // Non-retryable status
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(Failure::Rejected(format!(
                    "google translate HTTP error {}: {}",
                    status, body
                )));
            }
            Err(e) => {
                if attempt >= MAX_RETRIES {
                    return Err(Failure::Outage(e.to_string()));
                }
                sleep(backoff(attempt)).await;
                attempt += 1;
                continue;
            }
        }
    }
}

/// Pull the translation out of either Google payload shape:
///
/// * `translate_a/t` with `sl=<lang>`: `["你好"]`
/// * `translate_a/t` with `sl=auto`: `[["你好","ja"]]` (text + detected code)
/// * legacy `translate_a/single`: `[[["你好","こんにちは",…], …]]`, one entry per
///   sentence chunk, which is why the fragments are joined.
fn parse_google_payload(value: &Value) -> Option<String> {
    let entries = value.as_array()?;
    let first = entries.first()?;

    if let Some(text) = first.as_str() {
        return Some(text.to_string());
    }

    let chunks = first.as_array()?;
    // `[["translation","detected-language"]]`
    if let Some(text) = chunks.first().and_then(|chunk| chunk.as_str()) {
        return Some(text.to_string());
    }

    // `[[["fragment","source",…], ["fragment",…]]]`
    let joined: String = chunks
        .iter()
        .filter_map(|chunk| chunk.get(0).and_then(|fragment| fragment.as_str()))
        .collect();

    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

// ─── Microsoft (Azure AI Translator v3) ────────────────────────────────────

async fn microsoft_batch(
    client: &Client,
    texts: &[String],
    from: &str,
    to: &str,
    settings: &MicrosoftSettings,
) -> Vec<Result<String, Failure>> {
    let mut results: Vec<Result<String, Failure>> =
        (0..texts.len()).map(|_| Ok(String::new())).collect();

    let mut pending: Vec<usize> = Vec::new();
    for (index, text) in texts.iter().enumerate() {
        if text.trim().is_empty() {
            results[index] = Ok(String::new());
        } else {
            pending.push(index);
        }
    }

    if pending.is_empty() {
        return results;
    }

    if !settings.is_configured() {
        let message = "microsoft translator is selected but microsoft.key is empty in config.json";
        for index in pending {
            results[index] = Err(Failure::Fatal(message.to_string()));
        }
        return results;
    }

    // Target is mandatory for the API; an unknown value is rejected there and
    // degrades to "keep the original" like any other per-segment failure.
    let to_code = normalize_microsoft_lang(to).unwrap_or_else(|| "en".to_string());
    let from_code = normalize_microsoft_lang(from);

    for chunk in pending.chunks(MICROSOFT_CHUNK) {
        let payload: Vec<Value> = chunk
            .iter()
            .map(|index| json!({ "Text": texts[*index] }))
            .collect();

        match microsoft_request(client, settings, &payload, from_code.as_deref(), &to_code).await {
            Ok(translations) if translations.len() == chunk.len() => {
                for (index, translated) in chunk.iter().zip(translations) {
                    results[*index] = Ok(translated);
                }
            }
            Ok(translations) => {
                let message = format!(
                    "microsoft translator returned {} translations for {} inputs",
                    translations.len(),
                    chunk.len()
                );
                for index in chunk {
                    results[*index] = Err(Failure::Rejected(message.clone()));
                }
            }
            Err(failure) => {
                for index in chunk {
                    results[*index] = Err(failure.clone());
                }
            }
        }
    }

    results
}

async fn microsoft_request(
    client: &Client,
    settings: &MicrosoftSettings,
    payload: &[Value],
    from: Option<&str>,
    to: &str,
) -> Result<Vec<String>, Failure> {
    let endpoint = settings.endpoint.trim_end_matches('/');

    let mut attempt = 0u32;
    loop {
        let mut request = client
            .post(format!("{endpoint}/translate"))
            .query(&[("api-version", "3.0"), ("to", to)])
            .header("Ocp-Apim-Subscription-Key", settings.key.trim())
            .header("Content-Type", "application/json; charset=UTF-8")
            .json(&payload);

        if let Some(from) = from {
            request = request.query(&[("from", from)]);
        }
        if let Some(region) = settings
            .region
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        {
            request = request.header("Ocp-Apim-Subscription-Region", region);
        }

        match request.send().await {
            Ok(resp) => {
                let status = resp.status();

                if status.is_success() {
                    let body: Value = match resp.json().await {
                        Ok(body) => body,
                        Err(e) => {
                            return Err(Failure::Rejected(format!(
                                "microsoft translator response was not JSON: {e}"
                            )))
                        }
                    };
                    let Some(items) = body.as_array() else {
                        return Err(Failure::Rejected(
                            "microsoft translator response was not an array".to_string(),
                        ));
                    };
                    let translations = items
                        .iter()
                        .map(|item| {
                            item.get("translations")
                                .and_then(|t| t.get(0))
                                .and_then(|t| t.get("text"))
                                .and_then(|t| t.as_str())
                                .unwrap_or_default()
                                .to_string()
                        })
                        .collect();
                    return Ok(translations);
                }

                if status.as_u16() == 429 || status.is_server_error() {
                    if attempt >= MAX_RETRIES {
                        return Err(Failure::Outage(format!(
                            "microsoft translator HTTP {status} after retries"
                        )));
                    }
                    let wait = retry_after(&resp).unwrap_or_else(|| backoff(attempt));
                    sleep(wait).await;
                    attempt += 1;
                    continue;
                }

                let body = resp.text().await.unwrap_or_default();
                let message = format!("microsoft translator HTTP {status}: {body}");
                return Err(match status.as_u16() {
                    // Wrong key, wrong region or a wrong resource endpoint:
                    // retrying cannot fix any of those.
                    401 | 403 | 404 => Failure::Fatal(message),
                    _ => Failure::Rejected(message),
                });
            }
            Err(e) => {
                if attempt >= MAX_RETRIES {
                    return Err(Failure::Outage(e.to_string()));
                }
                sleep(backoff(attempt)).await;
                attempt += 1;
                continue;
            }
        }
    }
}

/// Regenerate `words` for a segment by splitting text on whitespace
/// and interpolating timestamps uniformly between segment.start and segment.end.
/// Words after the first are prefixed with a space so that the formatting layer
/// can reconstruct the original spacing when rendering.
pub fn regenerate_words_uniform(seg: &mut Segment) {
    // Split on Unicode whitespace; filter out empty tokens
    let tokens: Vec<&str> = seg
        .text
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .collect();

    let n = tokens.len();
    if n == 0 {
        seg.words = Some(Vec::new());
        return;
    }

    let start = seg.start;
    let end = seg.end.max(start); // guard against inverted times
    let dur = end - start;

    // Assign bounds so that words tile the interval [start, end].
    // Prefix words after the first with a space so the formatting layer
    // knows to insert inter-word spacing.
    let mut words = Vec::with_capacity(n);
    for (i, w) in tokens.into_iter().enumerate() {
        let t0 = start + dur * (i as f64) / (n as f64);
        let t1 = start + dur * ((i + 1) as f64) / (n as f64);
        let text = if i == 0 { w.to_string() } else { format!(" {}", w) };
        words.push(WordTimestamp { text, start: t0, end: t1, probability: None });
    }

    seg.words = Some(words);
}

// get_translate_languages moved to utils.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_translate_a_t_single_language() {
        let value: Value = serde_json::from_str(r#"["你好"]"#).unwrap();
        assert_eq!(parse_google_payload(&value).as_deref(), Some("你好"));
    }

    #[test]
    fn parses_translate_a_t_auto_detect() {
        // sl=auto adds the detected language next to the text.
        let value: Value = serde_json::from_str(r#"[["你好","ja"]]"#).unwrap();
        assert_eq!(parse_google_payload(&value).as_deref(), Some("你好"));
    }

    #[test]
    fn parses_legacy_gtx_sentence_chunks() {
        let value: Value =
            serde_json::from_str(r#"[[["你好，","こんにちは、",null,null],["世界。","世界。",null,null]]]"#)
                .unwrap();
        assert_eq!(parse_google_payload(&value).as_deref(), Some("你好，世界。"));
    }

    #[test]
    fn empty_translation_is_not_an_error() {
        let value: Value = serde_json::from_str(r#"[""]"#).unwrap();
        assert_eq!(parse_google_payload(&value).as_deref(), Some(""));
    }
}
