/**
 * Typed wrappers over the Tauri command layer — the frontend's only door to
 * the backend. The frontend never touches the disk directly.
 */
import { invoke, Channel } from "@tauri-apps/api/core";
import type {
  BackupArchive,
  BackupDirCheck,
  BackupInspection,
  BackupOutcome,
  BackupProgress,
  BackupRestoreOutcome,
  BackupStatus,
  BatchProgress,
  BatchRemoveReport,
  BatchScanReport,
  CollectionReport,
  DetectedEnvironment,
  FindAssetsReport,
  FindProgress,
  JslotRoot,
  LibraryEntry,
  LibraryListing,
  Mo2ProfileOption,
  MergedEntry,
  NexusValidation,
  PresetInspection,
  RateLimitInfo,
  RestoreReport,
  ScanResult,
  Settings,
  SingleRemoveOutcome,
  SnapshotInfo,
  SnapshotStats,
} from "./types";

// ---- settings -------------------------------------------------------------

export async function getSettings(): Promise<Settings> {
  return await invoke<Settings>("get_settings");
}

export async function saveSettings(settings: Settings): Promise<void> {
  await invoke("save_settings", { settings });
}

export async function defaultBackupDir(): Promise<string> {
  return await invoke<string>("default_backup_dir");
}

export async function validateBackupDir(path: string): Promise<BackupDirCheck> {
  return await invoke<BackupDirCheck>("validate_backup_dir", { path });
}

// ---- detection ------------------------------------------------------------

export async function detectEnvironment(): Promise<DetectedEnvironment> {
  return await invoke<DetectedEnvironment>("detect_environment");
}

export async function listMo2Profiles(modsDir: string): Promise<Mo2ProfileOption[]> {
  return await invoke<Mo2ProfileOption[]>("list_mo2_profiles", { modsDir });
}

/** The default scan roots for the given (possibly unsaved) settings. */
export async function defaultJslotRoots(settings: Settings): Promise<JslotRoot[]> {
  return await invoke<JslotRoot[]>("default_jslot_roots", { settings });
}

// ---- scanning -------------------------------------------------------------

export async function scanJslots(): Promise<ScanResult> {
  return await invoke<ScanResult>("scan_jslots");
}

// ---- backup ---------------------------------------------------------------

export async function getBackupStatus(): Promise<BackupStatus> {
  return await invoke<BackupStatus>("get_backup_status");
}

export async function runBackup(
  onProgress?: (p: BackupProgress) => void,
): Promise<BackupOutcome> {
  const channel = new Channel<BackupProgress>();
  if (onProgress) channel.onmessage = onProgress;
  return await invoke<BackupOutcome>("run_backup", { onProgress: channel });
}

export async function listBackups(): Promise<BackupArchive[]> {
  return await invoke<BackupArchive[]>("list_backups");
}

export async function inspectBackup(
  path: string,
  onProgress: (p: BackupProgress) => void,
): Promise<BackupInspection> {
  const channel = new Channel<BackupProgress>();
  channel.onmessage = onProgress;
  return await invoke<BackupInspection>("inspect_backup", { path, onProgress: channel });
}

export async function restoreBackup(
  path: string,
  entries: string[],
  onProgress: (p: BackupProgress) => void,
): Promise<BackupRestoreOutcome> {
  const channel = new Channel<BackupProgress>();
  channel.onmessage = onProgress;
  return await invoke<BackupRestoreOutcome>("restore_backup", { path, entries, onProgress: channel });
}

// ---- snapshots / history --------------------------------------------------

export async function listSnapshots(): Promise<SnapshotInfo[]> {
  return await invoke<SnapshotInfo[]>("list_snapshots");
}

export async function restoreSnapshot(id: string): Promise<RestoreReport> {
  return await invoke<RestoreReport>("restore_snapshot", { id });
}

export async function snapshotStats(): Promise<SnapshotStats> {
  return await invoke<SnapshotStats>("snapshot_stats");
}

export async function clearSnapshots(): Promise<void> {
  await invoke("clear_snapshots");
}

// ---- remove bodyslide -----------------------------------------------------

export async function inspectPreset(path: string): Promise<PresetInspection> {
  return await invoke<PresetInspection>("inspect_preset", { path });
}

export async function removeBodyMorphs(path: string): Promise<SingleRemoveOutcome> {
  return await invoke<SingleRemoveOutcome>("remove_body_morphs", { path });
}

export async function batchScan(
  onProgress?: (p: BatchProgress) => void,
): Promise<BatchScanReport> {
  const channel = new Channel<BatchProgress>();
  if (onProgress) channel.onmessage = onProgress;
  return await invoke<BatchScanReport>("batch_scan", { onProgress: channel });
}

export async function batchRemove(
  paths: string[],
  onProgress?: (p: BatchProgress) => void,
): Promise<BatchRemoveReport> {
  const channel = new Channel<BatchProgress>();
  if (onProgress) channel.onmessage = onProgress;
  return await invoke<BatchRemoveReport>("batch_remove", { paths, onProgress: channel });
}

// ---- nexus ----------------------------------------------------------------

export async function validateNexusKey(key: string): Promise<NexusValidation> {
  return await invoke<NexusValidation>("validate_nexus_key", { key });
}

export async function getRateLimit(): Promise<RateLimitInfo | null> {
  return await invoke<RateLimitInfo | null>("get_rate_limit");
}

// ---- find assets ----------------------------------------------------------

export async function findAssets(
  path: string,
  onProgress?: (p: FindProgress) => void,
): Promise<FindAssetsReport> {
  const channel = new Channel<FindProgress>();
  if (onProgress) channel.onmessage = onProgress;
  return await invoke<FindAssetsReport>("find_assets", { path, onProgress: channel });
}

// ---- asset library -------------------------------------------------------

export async function libraryList(): Promise<LibraryListing> {
  return await invoke<LibraryListing>("library_list");
}

export async function librarySaveEntries(entries: LibraryEntry[]): Promise<void> {
  await invoke("library_save_entries", { entries });
}

export async function libraryDeleteEntry(id: string): Promise<void> {
  await invoke("library_delete_entry", { id });
}

export async function libraryRestoreSeed(id: string): Promise<void> {
  await invoke("library_restore_seed", { id });
}

// ---- collection review -----------------------------------------------------

export async function reviewCollection(
  onProgress?: (p: FindProgress) => void,
): Promise<CollectionReport> {
  const channel = new Channel<FindProgress>();
  if (onProgress) channel.onmessage = onProgress;
  return await invoke<CollectionReport>("review_collection", { onProgress: channel });
}

export async function reviewRefresh(
  onProgress?: (p: FindProgress) => void,
): Promise<CollectionReport> {
  const channel = new Channel<FindProgress>();
  if (onProgress) channel.onmessage = onProgress;
  return await invoke<CollectionReport>("review_refresh", { onProgress: channel });
}

// ---- shared display helpers ----------------------------------------------

/** "2.4 KB" / "13.1 MB" style size. */
export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Local date + time from unix seconds. */
export function formatWhen(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}
