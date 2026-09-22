/**
 * Tiny cross-component signals: bumped whenever settings are saved, so pages
 * that cached settings-derived state (API key presence, scan roots, backup
 * status) know to refresh — the Settings dialog lives in the TitleBar and
 * outlives every route.
 */
import { getBackupStatus } from "$lib/tauri";
import type { BackupStatus } from "$lib/types";

export const appEvents = $state({ settingsVersion: 0, presetsVersion: 0 });

export function bumpSettings(): void {
  appEvents.settingsVersion++;
}

/** Files dropped on the window, waiting for the page that handles them.
 *  The page clears it once it has acted, so a drop that had to navigate
 *  first is still handled when the page mounts. */
export const drops = $state<{ pending: string[] | null }>({ pending: null });

/** Call after anything that writes presets or makes a backup — both change
 *  what the backup nudge reports. */
export function bumpPresets(): void {
  appEvents.presetsVersion++;
}

/** Backup coverage for the launch nudge and the sidebar dot. `dismissed`
 *  lasts for the session only. */
export const backupNudge = $state<{ status: BackupStatus | null; dismissed: boolean }>({
  status: null,
  dismissed: false,
});

export async function refreshBackupNudge(): Promise<void> {
  try {
    backupNudge.status = await getBackupStatus();
  } catch {
    // Keep the last known state; the Backup page reports errors in full.
  }
}

/** Presets on disk the last backup doesn't cover — all of them when there's
 *  no usable backup. Zero when there's nothing to back up or nowhere to put
 *  it: the Backup page explains those, and a nudge couldn't act on them. */
export function uncoveredCount(s: BackupStatus | null): number {
  if (!s || s.file_count === 0 || !s.destination) return 0;
  return s.changed_since_backup ?? s.file_count;
}
