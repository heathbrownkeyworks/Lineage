/**
 * TypeScript mirrors of the Rust command layer types (src-tauri/src).
 * Field names match the serde output exactly (snake_case).
 */

/** How the user manages their mods. */
export type ModManager = "manual" | "mo2" | "vortex";

/** How a configured root is scanned. */
export type RootKind = "plain" | "mo2_mods";

/** One root path scanned (always recursively) for .jslot files. */
export type JslotRoot = {
  id: string;
  label: string;
  path: string;
  kind: RootKind;
};

/** Persisted app settings (settings.json in the app config dir). */
export type Settings = {
  mod_manager: ModManager;
  skyrim_folder: string;
  mo2_instance: string;
  mo2_mods_folder: string;
  mo2_profile_dir: string;
  vortex_staging_folder: string;
  jslot_roots: JslotRoot[];
  backup_dir: string;
  snapshot_retention: number;
  nexus_api_key: string;
  last_backup_at: number | null;
  last_backup_path: string | null;
  setup_dismissed: boolean;
};

/** An MO2 profile discovered next to the mods folder. */
export type Mo2ProfileOption = {
  name: string;
  path: string;
};

/** Everything first-run detection could discover. All fields are suggestions. */
export type DetectedEnvironment = {
  manager: ModManager;
  skyrim_folder: string | null;
  mo2_instance: string | null;
  mo2_mods_folder: string | null;
  mo2_profile_dir: string | null;
  mo2_profiles: Mo2ProfileOption[];
  vortex_present: boolean;
  vortex_staging_folder: string | null;
};

/** Backup destination validation result. */
export type BackupDirCheck = {
  writable: boolean;
  problem: string | null;
  inside_mod_tree: string | null;
};

/** One discovered preset file. */
export type JslotFile = {
  path: string;
  file_name: string;
  root_id: string;
  root_label: string;
  rel_path: string;
  size: number;
  modified: number;
};

/** Per-root scan summary. */
export type RootScan = {
  root_id: string;
  label: string;
  path: string;
  kind: RootKind;
  exists: boolean;
  count: number;
};

export type ScanResult = {
  files: JslotFile[];
  roots: RootScan[];
  total: number;
};

/** Backup screen status. */
export type BackupStatus = {
  destination: string;
  file_count: number;
  total_size: number;
  last_backup_at: number | null;
  last_backup_path: string | null;
  last_backup_exists: boolean;
};

export type BackupProgress = {
  current: number;
  total: number;
  name: string;
};

export type BackupOutcome = {
  archive_path: string;
  file_count: number;
  total_size: number;
  backed_up_at: number;
};

/** One file inside a snapshot. */
export type SnapshotEntry = {
  entry: string;
  restore_to: string;
};

/** A snapshot as the History view sees it (meta is flattened). */
export type SnapshotInfo = {
  id: string;
  operation: string;
  created_at: number;
  entries: SnapshotEntry[];
  archive_path: string;
  archive_exists: boolean;
  size: number;
};

export type FailedFile = {
  path: string;
  reason: string;
};

export type RestoreReport = {
  restored: string[];
  missing_destination: string[];
  failed: FailedFile[];
};

export type SnapshotStats = {
  count: number;
  total_size: number;
  retention: number;
};

/** What one preset looks like to the Remove BodySlide features. */
export type PresetInspection = {
  path: string;
  file_name: string;
  section: string | null;
  morph_count: number;
  morph_names: string[];
  /** The section's JSON exactly as it appears in the file (single-file
   *  inspection only; null from batch scans). */
  section_json: string | null;
  parse_error: string | null;
  roundtrip_faithful: boolean;
};

export type RemovalDetail = {
  path: string;
  removed_section: string;
  removed_count: number;
};

export type SingleRemoveOutcome = {
  detail: RemovalDetail;
  snapshot_id: string;
};

export type BatchItem = {
  path: string;
  file_name: string;
  rel_path: string;
  root_label: string;
  morph_count: number;
};

export type BatchScanReport = {
  total: number;
  with_section: BatchItem[];
  without_section: number;
  failed: FailedFile[];
};

export type BatchProgress = {
  current: number;
  total: number;
  name: string;
  stage: "scanning" | "removing";
};

export type BatchRemoveReport = {
  snapshot_id: string;
  modified: RemovalDetail[];
  skipped: string[];
  failed: FailedFile[];
};

/** Nexus API key validation result. */
export type NexusValidation = {
  name: string;
  is_premium: boolean;
  daily_remaining: number | null;
  hourly_remaining: number | null;
};

export type RateLimitInfo = {
  daily_remaining: number | null;
  hourly_remaining: number | null;
  daily_reset: string | null;
  hourly_reset: string | null;
};

export type NexusModInfo = {
  mod_id: number;
  name: string | null;
  summary: string | null;
  author: string | null;
  uploaded_by: string | null;
  picture_url: string | null;
  version: string | null;
  category_id: number | null;
  available: boolean | null;
  category: string | null;
};

/** One reference extracted from a preset. */
export type AssetRef = {
  kind: "plugin" | "texture" | "morph";
  value: string;
  appeared_in: string[];
};

export type IdentifiedGroup = {
  mod_id: number | null;
  nexus: NexusModInfo | null;
  mod_folder: string | null;
  resolved_by: "meta.ini" | "md5" | "vortex-manifest" | "heuristic" | "local-folder" | "library";
  page_url: string | null;
  name: string | null;
  library_source: "seed" | "user" | null;
  assets: AssetRef[];
};

export type FindAssetsReport = {
  preset_path: string;
  identified: IdentifiedGroup[];
  unknown: AssetRef[];
  vanilla: string[];
  api_key_present: boolean;
  nexus_error: string | null;
  rate_limit: RateLimitInfo | null;
};

export type FindProgress = {
  stage: "parsing" | "resolving" | "nexus";
  current: number;
  total: number;
  detail: string;
};

/** One Asset Library mapping from a preset reference to a mod name + link. */
export type LibraryEntry = {
  id: string;
  kind: "plugin" | "texture" | "morph";
  pattern: string;
  match_type: "exact" | "prefix";
  name: string;
  url: string;
};

export type MergedEntry = LibraryEntry & {
  source: "seed" | "user";
  enabled: boolean;
};

export type LibraryListing = {
  entries: MergedEntry[];
  warning: string | null;
};
