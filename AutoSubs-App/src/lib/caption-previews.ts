import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { join } from "@tauri-apps/api/path";
import { ensureCaptionPreviewDir } from "@/api/resolve-api";

export const CAPTION_PREVIEW_DIR = "caption-previews";

/**
 * Preset preview images live in the app's portable data folder
 * (`<install dir>/AppData/caption-previews`).
 *
 * The file work happens in Rust: the fs plugin's capability scope is static and
 * cannot name a folder that depends on where the app was installed, so the
 * backend owns the move/delete and the asset-protocol scope.
 */

export async function storePresetPreview(
  presetId: string,
  renderedPath: string,
): Promise<string> {
  return invoke<string>("store_caption_preview", {
    presetId,
    sourcePath: renderedPath,
  });
}

export async function deletePresetPreview(filename: string): Promise<void> {
  try {
    await invoke("delete_caption_preview", { fileName: filename });
  } catch {
    // Preview cleanup is best-effort.
  }
}

export async function previewSrc(
  filename: string,
  cacheKey: string,
): Promise<string> {
  const dir = await ensureCaptionPreviewDir();
  const path = await join(dir, filename);
  return `${convertFileSrc(path)}?v=${encodeURIComponent(cacheKey)}`;
}
