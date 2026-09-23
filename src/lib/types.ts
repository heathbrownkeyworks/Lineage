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
  /** Presets on disk the last backup doesn't cover; null when there's no
   *  usable backup. */
  changed_since_backup: number | null;
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

/** A `JSLOT-BACKUP-*.zip` in the backup folder. */
export type BackupArchive = {
  path: string;
  file_name: string;
  size: number;
  /** File mtime, unix seconds. */
  modified: number;
  /** When the backup was taken — only known for archives with a manifest. */
  created_at: number | null;
  preset_count: number;
  has_manifest: boolean;
  /** False when the file isn't a readable zip. */
  readable: boolean;
};

/** How a preset in a backup compares with what's on disk now. Only
 *  `modified` and `missing` are restorable. */
export type BackupEntryStatus = "unchanged" | "modified" | "missing" | "folder_gone" | "unmapped";

export type BackupEntry = {
  /** Zip entry name — what gets sent back to restore it. */
  entry: string;
  file_name: string;
  /** Mod folder under an MO2 root, otherwise the root's label. */
  group: string;
  rel_path: string;
  target: string | null;
  status: BackupEntryStatus;
  size: number;
};

export type BackupInspection = {
  path: string;
  has_manifest: boolean;
  created_at: number | null;
  entries: BackupEntry[];
  /** Top-level archive folders no configured root answers to. */
  unmapped_roots: string[];
};

export type BackupRestoreOutcome = {
  restored: string[];
  unchanged: string[];
  folder_gone: string[];
  unmapped: string[];
  failed: FailedFile[];
  /** Snapshot of what the restore overwrote, for Undo. */
  snapshot_id: string | null;
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

/** What Clean Preset can remove. Face overlays are deliberately not one of
 *  these — they're the preset's look and are never touched. */
export type CleanCategory = "body_morphs" | "body_overlays" | "skeleton" | "weapon_camera" | "head_neck";

/** What one category holds in a preset. */
export type Finding = {
  category: CleanCategory;
  count: number;
  /** Morph names for body morphs, node names otherwise, in file order. */
  items: string[];
  /** The removed entries exactly as the file writes them — single-file
   *  inspection only; null from batch scans. */
  preview: string | null;
};

/** What one preset looks like to Clean Preset. */
export type PresetInspection = {
  path: string;
  file_name: string;
  findings: Finding[];
  parse_error: string | null;
  roundtrip_faithful: boolean;
};

export type CleanDetail = {
  path: string;
  removed: Finding[];
};

export type SingleCleanOutcome = {
  detail: CleanDetail;
  snapshot_id: string;
};

export type BatchItem = {
  path: string;
  file_name: string;
  rel_path: string;
  root_label: string;
  /** Entries per category, for filtering by the chosen ones without a rescan. */
  counts: Partial<Record<CleanCategory, number>>;
};

export type BatchScanReport = {
  total: number;
  candidates: BatchItem[];
  nothing_to_clean: number;
  failed: FailedFile[];
};

export type BatchProgress = {
  current: number;
  total: number;
  name: string;
  stage: "scanning" | "cleaning";
};

export type BatchCleanReport = {
  snapshot_id: string;
  cleaned: CleanDetail[];
  skipped: string[];
  failed: FailedFile[];
};

/** One mod a preset pack needs. */
export type Requirement = {
  key: string;
  name: string;
  url: string | null;
  /** Distinct chosen presets that need it. */
  used_by: number;
  resolved_by: string;
  /** Empty for RaceMenu, which no preset references and every one needs. */
  assets: AssetRef[];
};

export type UnknownReference = {
  asset: AssetRef;
  used_by: number;
};

export type RequirementsReport = {
  preset_count: number;
  failed: FailedFile[];
  requirements: Requirement[];
  /** Left out of the rendered list — shown so a pack never ships short. */
  unknown: UnknownReference[];
  api_key_present: boolean;
  nexus_error: string | null;
  library_warning: string | null;
};

export type ExportFormat = "bbcode" | "markdown" | "plain";

export type RenderLine = {
  name: string;
  url: string | null;
  used_by: number;
};

export type CompareSection =
  | "head_parts"
  | "vanilla_sliders"
  | "custom_sliders"
  | "sculpt"
  | "tints"
  | "face_textures"
  | "face_overlays"
  | "appearance"
  | "body";

export type ItemDiff = {
  key: string;
  left: string | null;
  right: string | null;
  same: boolean;
  /** e.g. "12 vertices differ". */
  note: string | null;
};

export type SectionDiff = {
  section: CompareSection;
  label: string;
  differences: number;
  items: ItemDiff[];
};

export type Comparison = {
  left: string;
  right: string;
  same_bytes: boolean;
  same_face: boolean;
  face_differences: number;
  body_differences: number;
  sections: SectionDiff[];
};

export type PresetRef = {
  path: string;
  file_name: string;
  modified: number;
};

export type DuplicateGroup = { presets: PresetRef[] };

export type NearTwin = {
  left: PresetRef;
  right: PresetRef;
  differences: number;
  what: string[];
};

export type DuplicatesReport = {
  total: number;
  exact: DuplicateGroup[];
  same_face: DuplicateGroup[];
  near_twins: NearTwin[];
  unreadable: FailedFile[];
};

export type CompareProgress = { current: number; total: number; detail: string };

export type RemoveOutcome = {
  snapshot_id: string;
  removed: string[];
  failed: FailedFile[];
};

export type ReadinessStatus = "missing" | "unconfirmed" | "ready";

/** One reference a preset uses, checked against this setup. */
export type ReadinessCheck = {
  kind: string;
  value: string;
  status: ReadinessStatus;
  /** Where it was found, or why not. */
  detail: string;
  source_name: string | null;
  source_url: string | null;
};

export type PresetReadiness = {
  preset_path: string;
  profile: string;
  /** Missing first, then unconfirmed, then ready. */
  checks: ReadinessCheck[];
  parse_error: string | null;
};

/** Everything one fix covers, and the presets it affects. */
export type ReadinessCause = {
  /** Unique with `status`; titles aren't. */
  key: string;
  status: ReadinessStatus;
  title: string;
  detail: string;
  references: string[];
  presets: string[];
  source_url: string | null;
};

export type SweepReport = {
  profile: string;
  total: number;
  /** Nothing missing. `ready + missing === total`. */
  ready: number;
  missing: number;
  /** Presets with an unconfirmed slider family, in either group. */
  unconfirmed: number;
  unreadable: FailedFile[];
  causes: ReadinessCause[];
};

export type ReadinessProgress = {
  stage: "indexing" | "parsing" | "checking";
  current: number;
  total: number;
  detail: string;
};

export type PackProgress = {
  current: number;
  total: number;
  name: string;
};

/** One file of a RaceMenu head export. */
export type ExportFile = { path: string; size: number };

/** A preset's head export: the .nif (head mesh) and .dds (tint) Export Head writes. */
export type HeadExport = {
  preset: string;
  nif: ExportFile | null;
  dds: ExportFile | null;
  /** Where to start looking by hand. */
  look_in: string;
};

/** What the page sends to pack for one preset. */
export type HeadExportChoice = { preset: string; nif: string | null; dds: string | null };

export type FolderMatch = {
  found: HeadExport[];
  /** Presets whose name turned up in more than one folder. */
  ambiguous: string[];
};

/** What ships for one preset in a pack. */
export type HeadRow = {
  nif: ExportFile | null;
  dds: ExportFile | null;
  /** found: by exact name; chosen: by hand or from a folder; none: nothing, or cleared. */
  how: "found" | "chosen" | "none";
  look_in: string;
};

export type PackOutcome = {
  path: string;
  presets: number;
  /** Presets that had something cleaned out of them. */
  cleaned: number;
  head_exports: number;
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
  /** Unique group identity from the backend — the only safe list key: two
   *  groups can share mod_id, name and URL (e.g. the ECE/CME slider pair). */
  key: string;
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
  library_warning: string | null;
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

/** One library entry's worth of unknown references — a mod's folder, a
 *  loose-file name prefix, or a morph family. Grouping is what makes the
 *  Unknown queue finishable: a mod with forty overlay textures is one
 *  decision, not forty. */
export type UnknownGroup = {
  key: string;
  kind: "plugin" | "texture" | "morph";
  pattern: string;
  match_type: "exact" | "prefix";
  assets: AssetRef[];
};

/** Every preset across the configured roots, resolved at once. */
export type CollectionReport = {
  total_presets: number;
  parse_failures: number;
  identified: IdentifiedGroup[];
  unknown: AssetRef[];
  /** `unknown`, collapsed into the library entries that would cover it. */
  unknown_groups: UnknownGroup[];
  vanilla: string[];
  /** "kind|lowercased value" → number of presets referencing it. */
  preset_counts: Record<string, number>;
  api_key_present: boolean;
  nexus_error: string | null;
  library_warning: string | null;
};
