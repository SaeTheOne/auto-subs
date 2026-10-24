import { invoke } from "@tauri-apps/api/core";

/**
 * Filesystem helpers for the app's own folders, backed by Rust commands.
 *
 * `@tauri-apps/plugin-fs` is scoped by static capability entries, which cannot
 * name the portable data folder (`<install dir>/AppData`): the runtime ACL
 * grant for it failed to deserialise on Windows. The backend therefore does the
 * work directly (`src-tauri/src/app_data_fs.rs`) and validates every path
 * against the portable folder plus the user folders the save/open dialogs hand
 * us.
 *
 * The signatures mirror the plugin's so call sites only change their import.
 */

export interface DirEntryInfo {
  name: string;
  isFile: boolean;
  isDirectory: boolean;
}

export interface FileStat {
  size: number;
  isFile: boolean;
  isDirectory: boolean;
  /** Plugin-compatible timestamp (the backend reports milliseconds). */
  mtime: Date | null;
}

interface RawFileStat {
  size: number;
  isFile: boolean;
  isDirectory: boolean;
  modifiedMs?: number | null;
}

export function exists(path: string): Promise<boolean> {
  return invoke<boolean>("app_fs_exists", { path });
}

export function mkdir(path: string, _options?: { recursive?: boolean }): Promise<void> {
  // The backend always creates parents, matching `{ recursive: true }`.
  return invoke("app_fs_mkdir", { path });
}

export function readTextFile(path: string): Promise<string> {
  return invoke<string>("app_fs_read_text", { path });
}

export function writeTextFile(path: string, contents: string): Promise<void> {
  return invoke("app_fs_write_text", { path, contents });
}

export function readDir(path: string): Promise<DirEntryInfo[]> {
  return invoke<DirEntryInfo[]>("app_fs_read_dir", { path });
}

export async function stat(path: string): Promise<FileStat> {
  const raw = await invoke<RawFileStat>("app_fs_stat", { path });
  return {
    size: raw.size,
    isFile: raw.isFile,
    isDirectory: raw.isDirectory,
    mtime: raw.modifiedMs ? new Date(raw.modifiedMs) : null,
  };
}

export function remove(path: string, options?: { recursive?: boolean }): Promise<void> {
  return invoke("app_fs_remove", { path, recursive: options?.recursive ?? false });
}

export function rename(from: string, to: string): Promise<void> {
  return invoke("app_fs_rename", { from, to });
}

export function copyFile(from: string, to: string): Promise<void> {
  return invoke("app_fs_copy", { from, to });
}
