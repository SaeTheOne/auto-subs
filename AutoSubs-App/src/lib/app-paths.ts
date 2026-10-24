import { invoke } from "@tauri-apps/api/core";

/**
 * Paths of the app's portable data folder.
 *
 * Everything AutoSubs writes lives under `<install dir>/AppData` instead of the
 * user's `%APPDATA%`, and `<install dir>/config.json` holds the translation
 * provider. The backend owns the platform-specific resolution (and the
 * fallback for an unwritable install directory), so the frontend asks for
 * absolute paths instead of building them.
 *
 * A path handed to `@tauri-apps/plugin-store` is resolved by Tauri against the
 * per-user app data dir when it is relative, so store filenames must be
 * requested through {@link dataFilePath}.
 */

let dataDirPromise: Promise<string> | null = null;
const filePathPromises = new Map<string, Promise<string>>();

/** Absolute path of `<install dir>/AppData`. */
export function appDataDir(): Promise<string> {
  if (!dataDirPromise) {
    dataDirPromise = invoke<string>("get_app_data_dir");
  }
  return dataDirPromise;
}

/** Absolute path of a file inside `<install dir>/AppData` (parents are created). */
export function dataFilePath(relativePath: string): Promise<string> {
  const cached = filePathPromises.get(relativePath);
  if (cached) return cached;

  const promise = invoke<string>("get_data_file", { relativePath }).catch((error) => {
    // Never cache a failure: a transient error (or a command registered later
    // in the startup sequence) must not poison every later call.
    filePathPromises.delete(relativePath);
    throw error;
  });

  filePathPromises.set(relativePath, promise);
  return promise;
}

/** Absolute path of `<install dir>/config.json`. */
export function appConfigPath(): Promise<string> {
  return invoke<string>("get_app_config_path");
}
