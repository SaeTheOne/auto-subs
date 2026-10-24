import * as React from "react";
import { useTranslation } from "react-i18next";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertCircle, CheckCircle2, FolderPlus, FilePlus2, Loader2, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Progress } from "@/components/ui/progress";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useSettingsStore } from "@/stores/settings-store";
import {
  BATCH_MEDIA_EXTENSIONS,
  type BatchItem,
  batchOutputName,
  runBatch,
  scanMediaFiles,
} from "@/lib/batch-runner";

/**
 * Batch transcription: drop folders (or pick them), transcribe every media file
 * inside one after another, and write the subtitles next to each file.
 *
 * Runs with the settings currently in the store (model, language, translate,
 * target language, text density...), so there is nothing else to configure here.
 */
export interface BatchSeed {
  /** Changes every time the user drops something, so repeats still trigger. */
  id: number;
  /** Already-scanned media paths. */
  paths: string[];
  autoStart: boolean;
}

export function BatchDialog({
  open: isOpen,
  onOpenChange,
  seed,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  seed?: BatchSeed | null;
}) {
  const { t } = useTranslation();
  const [items, setItems] = React.useState<BatchItem[]>([]);
  const [outputs, setOutputs] = React.useState<Record<string, string>>({});
  const [running, setRunning] = React.useState(false);
  const [summary, setSummary] = React.useState<{ done: number; failed: number } | null>(null);
  const cancelledRef = React.useRef(false);
  const handledSeedRef = React.useRef<number | null>(null);

  const translate = useSettingsStore((s) => s.translate);
  const targetLanguage = useSettingsStore((s) => s.targetLanguage);

  const addPaths = React.useCallback(
    async (rawPaths: string[]) => {
      const found: string[] = [];
      for (const path of rawPaths) {
        try {
          found.push(...(await scanMediaFiles(path)));
        } catch (error) {
          console.warn("Batch scan failed for", path, error);
        }
      }
      if (found.length === 0) return;

      setItems((current) => {
        const seen = new Set(current.map((item) => item.path.toLowerCase()));
        const additions = found
          .filter((path) => !seen.has(path.toLowerCase()))
          .map<BatchItem>((path) => ({ path, status: "pending" }));
        return [...current, ...additions];
      });

      // Pre-compute the output file name so the list shows where subtitles land.
      const settings = useSettingsStore.getState();
      const names: Record<string, string> = {};
      for (const path of found) {
        try {
          names[path] = await batchOutputName(path, settings);
        } catch {
          // Name is cosmetic; ignore.
        }
      }
      setOutputs((current) => ({ ...current, ...names }));
      setSummary(null);
    },
    [],
  );

  // Accept OS drops while the dialog is open.
  React.useEffect(() => {
    if (!isOpen) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;

    (async () => {
      const webview = await getCurrentWebview();
      const stop = await webview.onDragDropEvent((event: any) => {
        if (event.payload?.type !== "drop") return;
        const paths = event.payload.paths as string[] | undefined;
        if (paths?.length) void addPaths(paths);
      });
      if (disposed) stop();
      else unlisten = stop;
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [isOpen, addPaths]);

  const pickFolders = async () => {
    const picked = await open({ directory: true, multiple: true });
    if (!picked) return;
    await addPaths(Array.isArray(picked) ? picked : [picked]);
  };

  const pickFiles = async () => {
    const picked = await open({
      directory: false,
      multiple: true,
      filters: [{ name: "Media", extensions: BATCH_MEDIA_EXTENSIONS }],
    });
    if (!picked) return;
    await addPaths(Array.isArray(picked) ? picked : [picked]);
  };

  const runPaths = React.useCallback(async (paths: string[]) => {
    if (paths.length === 0) return;
    const settings = useSettingsStore.getState();
    cancelledRef.current = false;
    setRunning(true);
    setSummary(null);

    try {
      const result = await runBatch(paths, settings, {
        onStatus: (path, status, message) =>
          setItems((current) =>
            current.map((item) => (item.path === path ? { ...item, status, message } : item)),
          ),
        isCancelled: () => cancelledRef.current,
      });
      setSummary({ done: result.done, failed: result.failed });
    } catch (error) {
      console.error("Batch run failed:", error);
      setSummary({ done: 0, failed: paths.length });
    } finally {
      setRunning(false);
    }
  }, []);

  const start = () => runPaths(items.map((item) => item.path));

  // A drop on the main drop zone arrives as a seed: queue it and start right
  // away, so "drag the folder here" needs no second click.
  React.useEffect(() => {
    if (!isOpen || !seed) return;
    if (handledSeedRef.current === seed.id) return;
    if (running) return;
    handledSeedRef.current = seed.id;

    void (async () => {
      await addPaths(seed.paths);
      if (seed.autoStart) await runPaths(seed.paths);
    })();
  }, [isOpen, seed, running, addPaths, runPaths]);

  const cancel = async () => {
    cancelledRef.current = true;
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("cancel_transcription").catch(() => undefined);
  };

  const processed = items.filter((item) => item.status === "done" || item.status === "failed").length;
  const percent = items.length > 0 ? Math.round((processed / items.length) * 100) : 0;
  const finished = !running && items.length > 0 && processed === items.length;

  return (
    <Dialog open={isOpen} onOpenChange={(next) => !running && onOpenChange(next)}>
      <DialogContent className="max-w-2xl gap-3">
        <DialogHeader>
          <DialogTitle>{t("batch.title", "Batch transcription")}</DialogTitle>
          <DialogDescription>
            {t("batch.description", "Drop folders or files, subtitles are written next to each video.")}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" variant="outline" size="sm" disabled={running} onClick={pickFolders}>
            <FolderPlus />
            {t("batch.addFolder", "Add folder")}
          </Button>
          <Button type="button" variant="outline" size="sm" disabled={running} onClick={pickFiles}>
            <FilePlus2 />
            {t("batch.addFiles", "Add files")}
          </Button>
          <span className="text-xs text-muted-foreground">
            {t("batch.usingSettings", "Uses the current model and language settings")}
            {translate ? ` · → ${targetLanguage}` : ""}
          </span>
          {items.length > 0 && (
            <span className="ml-auto text-xs tabular-nums text-muted-foreground">
              {t("batch.count", "{{done}} / {{total}}", { done: processed, total: items.length })}
            </span>
          )}
        </div>

        {items.length === 0 ? (
          <div className="flex h-40 items-center justify-center rounded-md border border-dashed text-sm text-muted-foreground">
            {t("batch.empty", "No files yet. Drop folders here.")}
          </div>
        ) : (
          <ScrollArea className="h-64 rounded-md border">
            <ul className="divide-y">
              {items.map((item) => {
                const name = item.path.split(/[/\\]/).pop() ?? item.path;
                const folder = item.path.slice(0, Math.max(0, item.path.length - name.length - 1));
                return (
                  <li key={item.path} className="flex items-center gap-2 px-3 py-2">
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-sm">{name}</div>
                      <div className="truncate text-xs text-muted-foreground">
                        {outputs[item.path] ? `${folder}\\${outputs[item.path]}` : folder}
                      </div>
                      {item.status === "failed" && item.message && (
                        <div className="truncate text-xs text-destructive">{item.message}</div>
                      )}
                    </div>
                    <div className="shrink-0 text-xs text-muted-foreground">
                      {item.status === "running" && <Loader2 className="size-4 animate-spin" />}
                      {item.status === "done" && <CheckCircle2 className="size-4 text-emerald-500" />}
                      {item.status === "failed" && <AlertCircle className="size-4 text-destructive" />}
                      {item.status === "pending" && t("batch.pending", "Pending")}
                    </div>
                  </li>
                );
              })}
            </ul>
          </ScrollArea>
        )}

        {items.length > 0 && (
          <Progress value={running || summary ? percent : 0} className="h-1.5" />
        )}

        {summary && (
          <p className="text-xs text-muted-foreground">
            {summary.failed === 0
              ? t("batch.finished", "Done: {{done}} file(s)", { done: summary.done })
              : t("batch.finishedWithErrors", "Done: {{done}}, failed: {{failed}}", {
                  done: summary.done,
                  failed: summary.failed,
                })}
          </p>
        )}

        <DialogFooter className="gap-2">
          <Button
            type="button"
            variant="ghost"
            onClick={() => onOpenChange(false)}
            disabled={running}
          >
            {t("batch.close", "Close")}
          </Button>
          {running ? (
            <Button type="button" variant="destructive" onClick={cancel}>
              {t("batch.cancel", "Stop")}
            </Button>
          ) : (
            <Button type="button" onClick={start} disabled={items.length === 0}>
              <Play />
              {finished ? t("batch.runAgain", "Run again") : t("batch.start", "Start")}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
