# AutoSubs App

A cross-platform desktop app for generating subtitles with speaker diarization, translation, DaVinci Resolve integration, and Adobe Premiere Pro / After Effects integration via the bundled CEP extension — powered by AI transcription models running locally on your machine.

## Documentation

- **[CLI Guide](../CLI.md)** - Command-line interface reference
- **[Contributing Guide](../CONTRIBUTING.md)** - Development setup and contribution workflow
- **[AGENTS.md](../AGENTS.md)** - AI agent context with architecture gotchas and bridge details
- **[Resolve Integration](../Resolve-Integration/README.md)** - DaVinci Resolve integration architecture and development
- **[Adobe Extension](../Adobe-Extension/README.md)** - Adobe Premiere Pro/After Effects integration details

## Tech Stack

- **Frontend:** React + TypeScript (Vite)
- **Desktop Framework:** Tauri 2
- **Backend:** Rust (async via Tokio)
- **Transcription:** Whisper, Parakeet, Moonshine (via whisper-rs / ONNX Runtime)
- **Speaker Diarization:** Pyannote
- **Translation:** Google Translate API
- **Audio Processing:** FFmpeg (bundled sidecar)

## Architecture Overview

```mermaid
graph TB
    subgraph Frontend ["Frontend (React + TypeScript)"]
        UI[UI Components]
        CTX[State Contexts<br><i>Transcript, Models, Progress,<br>Settings, Integrations</i>]
        UI <--> CTX
    end

    subgraph Tauri ["Tauri Bridge"]
        IPC[IPC Commands + Events]
    end

    subgraph Backend ["Rust Backend"]
        API[Tauri Command Handlers]
        AP[Audio Preprocessor]
        API --> AP
    end

    subgraph Engine ["Transcription Engine Crate"]
        direction TB
        EM[Engine Manager]
        EM --> W[Whisper]
        EM --> P[Parakeet]
        EM --> M[Moonshine]
        EM --> D[Pyannote Diarization]
        EM --> FMT[Formatter<br><i>line breaking, timing, CPS</i>]
        EM --> TR[Translator]
    end

    subgraph External ["External"]
        FF[FFmpeg Sidecar]
        HF[HuggingFace Hub<br><i>model downloads</i>]
    end

    subgraph Resolve ["DaVinci Resolve-Integration (Optional)"]
        LUA[AutoSubs.lua Script]
    end

    subgraph Adobe ["Adobe Integration (Optional)"]
        WS[Adobe Bridge + Extension<br><i>WebSocket :8185</i>]
        PR[Premiere Pro]
        AE[After Effects]
        WS --> PR
        WS --> AE
    end

    Frontend <--> Tauri
    Tauri <--> Backend
    Backend --> Engine
    AP --> FF
    Engine --> HF
    LUA <-.-> Tauri
    Backend <--> WS
```

**How a transcription works end-to-end:**
1. User selects a file and clicks Transcribe
2. Rust backend preprocesses audio via FFmpeg (normalization, format conversion)
3. Transcription engine runs the chosen AI model (Whisper / Parakeet / Moonshine) locally
4. Optionally runs Pyannote for speaker diarization and Google Translate for translation
5. Formatter applies line-breaking, timing constraints, and language-specific rules
6. Results stream back to the UI in real time; user edits and exports

---

## Key Directories

| Directory | Purpose |
|---|---|
| `src/` | React frontend — components, contexts, hooks, utilities |
| `src/components/` | UI organized by feature (transcription, subtitles, settings, processing) |
| `src/contexts/` | Global state management (transcript, progress, models, settings, editor integrations) |
| `src-tauri/src/` | Rust backend — Tauri commands, audio preprocessing, logging |
| `src-tauri/crates/transcription-engine/` | Core engine — transcription, diarization, formatting, translation |
| `src-tauri/crates/transcription-engine/src/engines/` | Model-specific implementations (Whisper, Parakeet, Moonshine) |
| `src-tauri/resources/` | DaVinci Resolve Lua script, Adobe CEP extension resources, and subtitle templates |

## Data Locations

AutoSubs is portable: every file it writes lives next to the executable, under
`AppData/`, and the translation service is configured in `config.json` in the
install directory. Nothing is left in the user profile.

```
<install dir>/
  config.json                 # translation provider (google | microsoft)
  AppData/
    autosubs-store.json       # settings
    autosubs-presets.json     # user caption presets
    window-state.json
    Audio/                    # audio renders for the editor bridges
    Transcripts/              # saved subtitle documents
    caption-previews/         # preset thumbnails
    cache/                    # normalized audio
    logs/
    models/                   # downloaded AI models
    resolve-bridge/           # DaVinci Resolve file mailbox
    webview/                  # WebView2 profile (Windows)
```

- **Windows**: `%LOCALAPPDATA%\AutoSubs\AppData\models` for a normal per-user install
- **macOS**: `AutoSubs.app/Contents/AppData/models`
- **Linux / unwritable install dir**: falls back to the per-user folder
  (`~/.local/share/com.autosubs`, `%LOCALAPPDATA%\com.autosubs`)

Models are downloaded on first use and can be managed through the app's model
selection UI. See [docs/portable-appdata-and-translation.md](../docs/portable-appdata-and-translation.md)
for the full layout, the `config.json` schema and the Microsoft Translator setup.

## Development

For development setup, build commands, and platform-specific prerequisites, see the **[Contributing Guide](../CONTRIBUTING.md)**.

## Detailed Documentation

For in-depth architecture docs, component breakdowns, and to ask questions about the codebase, visit **[AutoSubs on DeepWiki](https://deepwiki.com/tmoroney/auto-subs)** — it provides detailed documentation with agentic search and Q&A.
