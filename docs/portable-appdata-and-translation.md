# Portable data folder and translation configuration

This build is **portable-first**: AutoSubs writes every file it owns next to
the executable instead of into the user profile, and the translation service is
configured from a plain `config.json` in the install directory.

Two things follow from that and are worth knowing before changing this code:

1. Anything that used to be reached through Tauri's `app_config_dir` /
   `app_data_dir` / `app_cache_dir` / `app_log_dir` now goes through
   `src-tauri/src/app_paths.rs`.
2. Anything that resolves a **relative** path through a Tauri API still lands in
   the per-user AppData, so relative paths are a bug in this build.

---

## 1. Where the files live

```
<install dir>/
  AutoSubs.exe
  config.json                 # translation provider (this doc, section 2)
  AppData/
    autosubs-store.json       # UI + processing settings (zustand persist)
    autosubs-presets.json     # user caption presets + preview mapping
    window-state.json         # window size/position/maximized
    Audio/                    # audio renders handed to the editor bridges
    Transcripts/              # saved subtitle documents + transcript index
    caption-previews/         # preset thumbnails rendered in Resolve
    cache/                    # normalized_audio.wav
    logs/                     # daily autosubs.log files
    models/                   # whisper / parakeet / diarize / aligner downloads
    resolve-bridge/           # request/response mailbox shared with the Lua loop
    webview/                  # WebView2 user data folder (profile, cache, cookies)
```

On Windows a normal (per-user) install lives in `%LOCALAPPDATA%\AutoSubs`, so
`AppData` is writable without elevation.

**Fallback.** A machine-wide install (`Program Files`) or a read-only package
path cannot host the folder. `app_paths::data_root()` then falls back to the old
per-user location (`%LOCALAPPDATA%\com.autosubs`, `~/.local/share/com.autosubs`,
`~/Library/Application Support/com.autosubs`) and logs it. That fallback
deliberately matches the env-derived mailbox path in `bootstrap.lua`, so the
Resolve bridge keeps working even then.

**Migration.** On first launch with a writable install directory:

* `autosubs-store.json` and `autosubs-presets.json` are copied over;
* the `models` folder is renamed (not re-downloaded);
* `Transcripts` files are moved across, so the saved-subtitle library survives;
* the plugin-era `.window-state.json` is deleted.

Logs, previews and normalized audio are not migrated: they are regenerated.

### How each piece is wired

| Concern | Owner | Notes |
| --- | --- | --- |
| Settings store | `src/stores/settings-store.ts` and `src/lib/tauri-storage.ts` | `load()` gets an **absolute** path from the `get_data_file` command |
| Presets store | `src/contexts/PresetsContext.tsx` | same absolute-path rule |
| Window geometry | `src-tauri/src/window_state.rs` | replaces `tauri-plugin-window-state` |
| Logs | `src-tauri/src/logging.rs` | `logs/` via `app_paths::subdir` |
| Models | `src-tauri/src/models.rs` | `models/` |
| Normalized audio | `src-tauri/src/transcription_api.rs` | `cache/` |
| Previews | `src-tauri/src/caption_previews.rs` | Rust does the move/delete so the static fs scope is irrelevant; the folder is added to the asset scope at runtime |
| Resolve mailbox | `src-tauri/src/resolve_bridge.rs` | `<install dir>/AppData/resolve-bridge`, baked into the installed launcher by `resolve_scripts.rs` |
| WebView2 profile | `src-tauri/src/main.rs` | `WebviewWindowBuilder::data_directory` (Windows) |
| Transcripts / Audio | `src/utils/file-utils.ts` | `fs` plugin, authorised by a **runtime capability** (below) |

The dev Resolve launcher gets the same mailbox path baked in by
`npm run setup-resolve` (`scripts/setup-resolve-dev.js`), because a dev binary's
data folder is `src-tauri/target/debug/AppData`.

### Why a runtime capability

The frontend keeps using `@tauri-apps/plugin-fs` for transcript documents, and
that plugin checks the ACL scope on every call. The static capability in
`src-tauri/capabilities/default.json` can only name compile-time paths
(`$APPLOCALDATA`, `$DOCUMENT`, ...), and there is no scope variable for "the
folder next to the executable" on Windows or macOS (`$EXE` is a Linux bin dir).

So `src-tauri/src/app_fs_scope.rs` adds one capability at runtime
(`dynamic-acl`, a default Tauri feature) whose scope is exactly
`<install dir>/AppData`. It grants only the nine `fs` permissions the frontend
uses there. Do not widen the static capability to `**` instead.

---

## 2. `config.json` - translation provider

Created with defaults on first launch. Edited by hand; changes apply to the next
translation batch (the loader watches size + mtime), no restart needed.

```json
{
  "translation": {
    "provider": "google",
    "fallback": null,
    "concurrency": 2,
    "google": {
      "endpoint": "https://translate.googleapis.com/translate_a/single"
    },
    "microsoft": {
      "endpoint": "https://api.cognitive.microsofttranslator.com",
      "key": "",
      "region": ""
    }
  }
}
```

The translation block may also sit at the top level of the file (no
`"translation"` wrapper); unknown keys are ignored so the file can grow other
sections later.

| Key | Meaning |
| --- | --- |
| `provider` | `google` (default, no credentials) or `microsoft` |
| `fallback` | optional second provider, tried once when the first fails service-wide |
| `concurrency` | parallel Google requests; clamped to 1..16 (Microsoft batches instead) |
| `google.endpoint` | override for a mirror/proxy |
| `microsoft.endpoint` | Azure Translator resource endpoint |
| `microsoft.key` | Azure Translator subscription key (`Ocp-Apim-Subscription-Key`) |
| `microsoft.region` | resource region, e.g. `eastasia` (`Ocp-Apim-Subscription-Region`) |

### Switching to Microsoft

1. Create an **Azure AI Translator** resource in the Azure portal.
2. Copy *Keys and Endpoint*: key 1 (or 2) and the region name.
3. In `<install dir>/config.json` set `translation.provider` to `microsoft`,
   paste the key into `translation.microsoft.key` and the region into
   `translation.microsoft.region`.
4. Optionally set `"fallback": "google"` so a 401/403 (wrong key) or an outage
   still produces translated captions.
5. Save the file. The next transcription's translation step uses it.

`provider: "microsoft"` with an empty key is not an error: the loader logs a
warning and keeps using Google, so a half-edited config never breaks the app.
`get_translation_status` and `reload_translation_config` commands expose the
resolved provider and the file path for diagnostics (the Settings UI does not
surface them yet).

### Request shapes

* **Google**: one request per segment, existing retry/backoff policy
  (429/5xx/transport, 5 attempts, jittered).
* **Microsoft**: `POST {endpoint}/translate?api-version=3.0&to=...[&from=...]`
  with a JSON array of `{ "Text": "..." }` (25 per chunk), honouring
  `Retry-After` on 429. Source `auto` omits `from`, so the service
  auto-detects.

### Failure policy (unchanged from the Google-only build)

* a segment that keeps failing keeps its **original text**;
* the batch errors only when *every* non-empty segment failed, i.e. the service
  is down or the credentials are wrong;
* `fallback` is tried once for that service-wide case.

### Language codes

Whisper emits ISO-639-1. `translation_client.rs` maps the exceptions for
Microsoft: `zh` to `zh-Hans`, `zh-TW`/`yue` to `zh-Hant`, `jw` to `jv`,
`no`/`nn` to `nb`, `tl` to `fil`, `iw` to `he`, `in` to `id`. Google keeps its
existing mapping (`jw` to `jv`, `yue` to `zh-TW`).
