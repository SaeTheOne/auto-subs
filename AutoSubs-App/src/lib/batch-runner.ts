import { invoke } from "@tauri-apps/api/core";
import { basename, dirname, join } from "@tauri-apps/api/path";
import { generateSrt } from "@/utils/srt-utils";
import { getActiveCensorWords } from "@/censor/merge";
import { models } from "@/lib/models";
import { writeTextFile } from "@/lib/app-fs";
import type {
  EnsureModelsRequest,
  EnsureModelsResponse,
  Settings,
  TranscriptionOptions,
} from "@/types";

/**
 * Batch transcription: transcribe a list of media files one after another and
 * drop the subtitles next to each file, with no manual export step.
 *
 * Outputs per file (source file `...\Episode 01.mkv`):
 *   Episode 01.orig.srt          source-language subtitles, when translating
 *   Episode 01.<target>.srt      translated subtitles, when translating
 *   Episode 01.srt               subtitles, when not translating
 *
 * The source SRT is written by the backend from the pre-translation segments
 * (`originalSrtPath`); the other one is generated here from the finished,
 * formatted segments so it matches what the app would export.
 */

export type BatchStatus = "pending" | "running" | "done" | "failed";

export interface BatchItem {
  path: string;
  status: BatchStatus;
  message?: string;
}

export interface BatchHooks {
  onStatus: (path: string, status: BatchStatus, message?: string) => void;
  isCancelled: () => boolean;
}

export interface BatchResult {
  done: number;
  failed: number;
  cancelled: boolean;
}

/** Recursively collect media files under `root` (a file path is fine too). */
export async function scanMediaFiles(root: string): Promise<string[]> {
  return invoke<string[]>("scan_media_files", { root });
}

/** Extensions offered in the batch file picker. */
export const BATCH_MEDIA_EXTENSIONS = [
  "mkv",
  "mp4",
  "mov",
  "avi",
  "m4v",
  "webm",
  "wmv",
  "mpg",
  "mpeg",
  "ts",
  "m2ts",
  "mts",
  "flv",
  "mp3",
  "wav",
  "m4a",
  "aac",
  "flac",
  "ogg",
  "opus",
  "wma",
  "aiff",
];

function buildOptions(
  audioPath: string,
  settings: Settings,
  modelPaths: EnsureModelsResponse,
  originalSrtPath?: string,
): TranscriptionOptions {
  const speakerLimit =
    typeof settings.maxSpeakers === "number" && !Number.isNaN(settings.maxSpeakers)
      ? Math.floor(settings.maxSpeakers) || null
      : null;

  const customDensity = settings.textDensity === "custom";

  return {
    audioPath,
    offset: 0,
    model: models[settings.model]?.value ?? "small",
    lang: settings.language,
    translate: settings.translate,
    targetLanguage: settings.targetLanguage,
    enableDtw: settings.enableDTW,
    enableVad: settings.enableVad,
    enableForcedAlignment: settings.enableForcedAlignment,
    enableGpu: settings.enableGpu,
    enableDiarize: settings.enableDiarize,
    maxSpeakers: speakerLimit,
    diarizeBackend: settings.diarizeBackend,
    density: settings.textDensity,
    maxLines: Math.max(1, Math.floor(settings.maxLinesPerSubtitle ?? 1)),
    customMaxCharsPerLine:
      customDensity && settings.customDensityUnit !== "words"
        ? Math.max(1, Math.floor(settings.customMaxCharsPerLine ?? 1))
        : undefined,
    customMaxWordsPerLine:
      customDensity && settings.customDensityUnit === "words"
        ? Math.max(1, Math.floor(settings.customMaxWordsPerLine ?? 1))
        : undefined,
    textCase: settings.textCase,
    removePunctuation: settings.removePunctuation,
    censoredWords: settings.enableCensor ? getActiveCensorWords(settings) : [],
    censorStyle: settings.censorStyle,
    customPrompt: settings.customPrompt.trim() || undefined,
    originalSrtPath,
    asrModelPath: modelPaths.asrModelPath,
    vadModelPath: modelPaths.vadModelPath,
    diarizeSegmentPath: modelPaths.diarizeSegmentPath,
    diarizeEmbeddingPath: modelPaths.diarizeEmbeddingPath,
    diarizeSortformerPath: modelPaths.diarizeSortformerPath,
    alignerModelDir: modelPaths.alignerDir,
  } as TranscriptionOptions;
}

/** Output name for one file, shown in the list before the run starts. */
export async function batchOutputName(path: string, settings: Settings): Promise<string> {
  const stem = (await basename(path)).replace(/\.[^.]+$/, "");
  return settings.translate ? `${stem}.${settings.targetLanguage}.srt` : `${stem}.srt`;
}

export async function runBatch(
  paths: string[],
  settings: Settings,
  hooks: BatchHooks,
): Promise<BatchResult> {
  let done = 0;
  let failed = 0;

  if (paths.length === 0) {
    return { done, failed, cancelled: false };
  }

  const speakerLimit =
    typeof settings.maxSpeakers === "number" && !Number.isNaN(settings.maxSpeakers)
      ? Math.floor(settings.maxSpeakers) || null
      : null;

  // Model paths are resolved once for the whole run: `ensure_models` checks and
  // downloads the model set for the current settings.
  const request: EnsureModelsRequest = {
    model: models[settings.model]?.value ?? "small",
    enableVad: settings.enableVad,
    enableDiarize: settings.enableDiarize,
    enableForcedAlignment: settings.enableForcedAlignment,
    maxSpeakers: speakerLimit,
    diarizeBackend: settings.diarizeBackend,
  };
  const modelPaths = await invoke<EnsureModelsResponse>("ensure_models", { request });

  for (const path of paths) {
    if (hooks.isCancelled()) {
      return { done, failed, cancelled: true };
    }

    hooks.onStatus(path, "running");
    try {
      const folder = await dirname(path);
      const stem = (await basename(path)).replace(/\.[^.]+$/, "");

      // The backend writes the source-language subtitles while it still has
      // them; only useful (and only requested) when the text gets translated.
      const originalSrtPath = settings.translate
        ? await join(folder, `${stem}.orig.srt`)
        : undefined;

      const options = buildOptions(path, settings, modelPaths, originalSrtPath);
      const transcript = await invoke<any>("transcribe_audio", { options });

      if (hooks.isCancelled()) {
        return { done, failed, cancelled: true };
      }

      const srt = generateSrt(transcript?.segments ?? [], {
        includeSpeakerLabels: settings.enableDiarize,
        speakers: transcript?.speakers ?? [],
      });

      if (!srt || srt.trim() === "") {
        throw new Error("no subtitles were produced");
      }

      const outputName = settings.translate
        ? `${stem}.${settings.targetLanguage}.srt`
        : `${stem}.srt`;
      await writeTextFile(await join(folder, outputName), srt);

      done += 1;
      hooks.onStatus(path, "done", outputName);
    } catch (error) {
      failed += 1;
      const message = error instanceof Error ? error.message : String(error);
      hooks.onStatus(path, "failed", message);
    }
  }

  return { done, failed, cancelled: hooks.isCancelled() };
}
