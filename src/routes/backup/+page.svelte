<script lang="ts">
  import { Archive, FolderOpen, Settings2, CheckCircle2 } from "lucide-svelte";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import PageHeader from "$lib/components/PageHeader.svelte";
  import ProgressBar from "$lib/components/ProgressBar.svelte";
  import SettingsDialog from "$lib/components/SettingsDialog.svelte";
  import { getBackupStatus, runBackup, formatSize, formatWhen } from "$lib/tauri";
  import { appEvents } from "$lib/stores/app.svelte";
  import type { BackupOutcome, BackupProgress, BackupStatus } from "$lib/types";

  let status = $state<BackupStatus | null>(null);
  let loading = $state(false);
  let running = $state(false);
  let progress = $state<BackupProgress | null>(null);
  let outcome = $state<BackupOutcome | null>(null);
  let error = $state<string | null>(null);
  let settingsOpen = $state(false);

  async function refresh() {
    loading = true;
    error = null;
    try {
      status = await getBackupStatus();
    } catch (e) {
      error = typeof e === "string" ? e : String(e);
    } finally {
      loading = false;
    }
  }

  async function backUpNow() {
    running = true;
    error = null;
    outcome = null;
    progress = null;
    try {
      outcome = await runBackup((p) => (progress = p));
      await refresh();
    } catch (e) {
      error = typeof e === "string" ? e : String(e);
    } finally {
      running = false;
      progress = null;
    }
  }

  async function showInFolder(path: string) {
    try {
      await revealItemInDir(path);
    } catch {}
  }

  // Initial load + reload whenever settings are saved anywhere (destination
  // or roots may have changed).
  $effect(() => {
    void appEvents.settingsVersion;
    void refresh();
  });
</script>

<PageHeader
  title="Backup"
  subtitle="One action that archives every JSLOT file across your configured locations into a single dated zip — the safety net for everything else Lineage does."
/>

<div class="content">
  {#if error}
    <div class="note note-danger">{error}</div>
  {/if}

  <div class="grid">
    <section class="sf-card pad stats">
      <h2 class="sf-label">What will be archived</h2>
      {#if loading && !status}
        <p class="quiet">Scanning…</p>
      {:else if status}
        <div class="stat-row">
          <div class="stat">
            <span class="big mono">{status.file_count}</span>
            <span class="stat-label">JSLOT files</span>
          </div>
          <div class="stat">
            <span class="big mono">{formatSize(status.total_size)}</span>
            <span class="stat-label">total size</span>
          </div>
        </div>
        <div class="dest">
          <span class="sf-label">Destination</span>
          <div class="dest-row">
            <span class="mono path" title={status.destination}>
              {status.destination || "No backup folder set"}
            </span>
            <button type="button" class="btn btn-ghost btn-sm" onclick={() => (settingsOpen = true)}>
              <Settings2 size={13} /> Change
            </button>
          </div>
        </div>
        <div class="last">
          <span class="sf-label">Last backed up</span>
          {#if status.last_backup_at && status.last_backup_exists}
            <p class="when">{formatWhen(status.last_backup_at)}</p>
            <p class="mono archive" title={status.last_backup_path}>{status.last_backup_path}</p>
          {:else if status.last_backup_at && !status.last_backup_exists}
            <p class="when warn">
              A backup was recorded on {formatWhen(status.last_backup_at)}, but the archive is no
              longer where it was saved — treat this as never backed up.
            </p>
          {:else}
            <p class="when warn">Never</p>
          {/if}
        </div>
      {/if}
    </section>

    <section class="sf-card pad action-panel">
      <h2 class="sf-label">Back up now</h2>
      <p class="explain">
        Presets are stored with their folder structure intact and grouped by location, so any
        file can be traced back to the mod it came from and restored to the right place.
        Existing archives are never overwritten.
      </p>
      {#if running}
        <ProgressBar
          current={progress?.current ?? 0}
          total={progress?.total ?? status?.file_count ?? 0}
          label={progress?.name ?? "Preparing…"}
        />
      {:else}
        <button
          type="button"
          class="btn btn-primary big-action"
          disabled={loading || !status || status.file_count === 0 || !status.destination}
          onclick={backUpNow}
        >
          <Archive size={15} /> Back up now
        </button>
        {#if status && status.file_count === 0}
          <p class="quiet">Nothing to archive yet — no presets were found in your locations.</p>
        {/if}
        {#if status && !status.destination}
          <p class="quiet">Set a backup destination in Settings first.</p>
        {/if}
      {/if}

      {#if outcome}
        <div class="note note-success success">
          <CheckCircle2 size={15} />
          <div>
            <p class="success-line">
              Backed up {outcome.file_count} presets ({formatSize(outcome.total_size)}) to
              <span class="mono">{outcome.archive_path.split(/[\\/]/).pop()}</span>
            </p>
            <button type="button" class="btn btn-ghost btn-sm" onclick={() => showInFolder(outcome!.archive_path)}>
              <FolderOpen size={13} /> Show in folder
            </button>
          </div>
        </div>
      {/if}
    </section>
  </div>
</div>

<SettingsDialog bind:open={settingsOpen} />

<style>
  .content {
    padding: 20px 28px 28px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 20px;
    align-items: start;
  }
  .pad {
    padding: 18px 20px;
  }
  .stats h2,
  .action-panel h2 {
    margin: 0 0 12px;
  }
  .quiet {
    color: var(--sf-text-3);
    font-size: 12.5px;
  }
  .stat-row {
    display: flex;
    gap: 32px;
    margin-bottom: 18px;
  }
  .stat {
    display: flex;
    flex-direction: column;
  }
  .big {
    font-size: 26px;
    color: var(--sf-secondary);
    font-family: var(--sf-font-mono);
    font-weight: 500;
  }
  .stat-label {
    font-size: 11px;
    color: var(--sf-text-3);
    text-transform: uppercase;
    letter-spacing: 0.1em;
  }
  .dest,
  .last {
    margin-top: 14px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .dest-row {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .path {
    flex: 1;
    min-width: 0;
    font-size: 11.5px;
    color: var(--sf-text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mono {
    font-family: var(--sf-font-mono);
  }
  .when {
    margin: 0;
    font-size: 13px;
    color: var(--sf-text);
  }
  .when.warn {
    color: var(--sf-warning);
  }
  .archive {
    margin: 0;
    font-size: 10.5px;
    color: var(--sf-text-off);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .explain {
    margin: 0 0 16px;
    font-size: 12.5px;
    color: var(--sf-text-3);
    line-height: 1.55;
  }
  .big-action {
    height: var(--sf-h-lg);
    padding: 0 24px;
    font-size: 14px;
  }
  .success {
    margin-top: 16px;
    display: flex;
    gap: 10px;
    align-items: flex-start;
  }
  .success-line {
    margin: 0 0 8px;
  }
</style>
