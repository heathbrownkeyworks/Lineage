<script lang="ts">
  import { Archive, ArchiveRestore, FolderOpen, Settings2, CheckCircle2, Undo2 } from "lucide-svelte";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import PageHeader from "$lib/components/PageHeader.svelte";
  import ProgressBar from "$lib/components/ProgressBar.svelte";
  import SettingsDialog from "$lib/components/SettingsDialog.svelte";
  import {
    getBackupStatus,
    runBackup,
    listBackups,
    inspectBackup,
    restoreBackup,
    restoreSnapshot,
    formatSize,
    formatWhen,
  } from "$lib/tauri";
  import { appEvents } from "$lib/stores/app.svelte";
  import type {
    BackupArchive,
    BackupEntry,
    BackupEntryStatus,
    BackupInspection,
    BackupOutcome,
    BackupProgress,
    BackupRestoreOutcome,
    BackupStatus,
    RestoreReport,
  } from "$lib/types";

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
      const [s] = await Promise.all([getBackupStatus(), loadArchives()]);
      status = s;
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

  // ---- Restore --------------------------------------------------------------

  const STATUS_ORDER: BackupEntryStatus[] = ["modified", "missing", "folder_gone", "unmapped", "unchanged"];
  const STATUS_LABEL: Record<BackupEntryStatus, string> = {
    modified: "Modified",
    missing: "Missing",
    folder_gone: "Folder gone",
    unmapped: "Unmapped",
    unchanged: "Unchanged",
  };
  const isRestorable = (e: BackupEntry) => e.status === "modified" || e.status === "missing";
  const errorText = (e: unknown) => (typeof e === "string" ? e : String(e));
  const fileNameOf = (path: string) => path.split(/[\\/]/).pop() ?? path;

  let archives = $state<BackupArchive[]>([]);
  let inspection = $state<BackupInspection | null>(null);
  let inspecting = $state(false);
  let inspectProgress = $state<BackupProgress | null>(null);
  let showUnchanged = $state(false);
  let selected = $state<Set<string>>(new Set());
  let confirming = $state(false);
  let restoring = $state(false);
  let restoreProgress = $state<BackupProgress | null>(null);
  let restoreOutcome = $state<BackupRestoreOutcome | null>(null);
  let undoing = $state(false);
  let undone = $state<RestoreReport | null>(null);
  let restoreError = $state<string | null>(null);

  const counts = $derived.by(() => {
    const c: Record<BackupEntryStatus, number> = {
      modified: 0,
      missing: 0,
      folder_gone: 0,
      unmapped: 0,
      unchanged: 0,
    };
    for (const e of inspection?.entries ?? []) c[e.status]++;
    return c;
  });
  const restorable = $derived((inspection?.entries ?? []).filter(isRestorable));
  const notRestorable = $derived(
    (inspection?.entries ?? []).filter((e) => e.status === "folder_gone" || e.status === "unmapped"),
  );
  /** Grouped rows: everything restorable, plus the unchanged ones on request.
   *  With ~1,800 unchanged presets, showing them by default would bury the
   *  handful that actually differ. */
  const groups = $derived.by(() => {
    const byGroup = new Map<string, BackupEntry[]>();
    for (const e of inspection?.entries ?? []) {
      if (!isRestorable(e) && !(showUnchanged && e.status === "unchanged")) continue;
      const list = byGroup.get(e.group);
      if (list) list.push(e);
      else byGroup.set(e.group, [e]);
    }
    return [...byGroup.entries()]
      .map(([name, entries]) => ({ name, entries }))
      .sort((a, b) => a.name.localeCompare(b.name));
  });
  const selectedOverwrites = $derived(
    restorable.filter((e) => e.status === "modified" && selected.has(e.entry)).length,
  );
  const selectedRecreates = $derived(
    restorable.filter((e) => e.status === "missing" && selected.has(e.entry)).length,
  );

  async function loadArchives() {
    try {
      archives = await listBackups();
    } catch (e) {
      restoreError = errorText(e);
    }
  }

  /** Load (or reload) a comparison. Selection resets to everything
   *  restorable — the confirm dialog is the deliberate step, as on Batch. */
  async function loadInspection(path: string) {
    inspecting = true;
    inspectProgress = null;
    try {
      inspection = await inspectBackup(path, (p) => (inspectProgress = p));
      selected = new Set(inspection.entries.filter(isRestorable).map((e) => e.entry));
    } catch (e) {
      restoreError = errorText(e);
    } finally {
      inspecting = false;
      inspectProgress = null;
    }
  }

  async function compare(path: string) {
    restoreError = null;
    restoreOutcome = null;
    undone = null;
    await loadInspection(path);
  }

  async function openOther() {
    const picked = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "Backup archive", extensions: ["zip"] }],
    });
    if (typeof picked === "string") await compare(picked);
  }

  function toggle(entry: string) {
    const next = new Set(selected);
    if (next.has(entry)) next.delete(entry);
    else next.add(entry);
    selected = next;
  }

  function toggleGroup(entries: BackupEntry[]) {
    const ids = entries.filter(isRestorable).map((e) => e.entry);
    const allOn = ids.every((id) => selected.has(id));
    const next = new Set(selected);
    for (const id of ids) {
      if (allOn) next.delete(id);
      else next.add(id);
    }
    selected = next;
  }

  function toggleAll() {
    selected =
      selected.size === restorable.length ? new Set() : new Set(restorable.map((e) => e.entry));
  }

  async function runRestore() {
    if (!inspection) return;
    confirming = false;
    restoring = true;
    restoreError = null;
    undone = null;
    restoreProgress = null;
    const path = inspection.path;
    try {
      restoreOutcome = await restoreBackup(path, [...selected], (p) => (restoreProgress = p));
      await loadInspection(path);
    } catch (e) {
      restoreError = errorText(e);
    } finally {
      restoring = false;
      restoreProgress = null;
    }
  }

  async function undoRestore() {
    const id = restoreOutcome?.snapshot_id;
    if (!id || !inspection) return;
    undoing = true;
    restoreError = null;
    try {
      undone = await restoreSnapshot(id);
      await loadInspection(inspection.path);
    } catch (e) {
      restoreError = errorText(e);
    } finally {
      undoing = false;
    }
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
  <section class="sf-card pad restore">
    <div class="restore-head">
      <h2 class="sf-label">Restore from a backup</h2>
      <button
        type="button"
        class="btn btn-ghost btn-sm"
        disabled={inspecting || restoring}
        onclick={openOther}
      >
        <FolderOpen size={13} /> Open another archive…
      </button>
    </div>
    <p class="explain">
      Compare a backup with what's on disk now, then put back the presets that changed. Anything a
      restore overwrites is snapshotted first, so it can be undone from History.
    </p>

    {#if restoreError}
      <div class="note note-danger">{restoreError}</div>
    {/if}

    {#if archives.length === 0}
      <p class="quiet">No backups in the backup folder yet.</p>
    {:else}
      <ul class="archive-list">
        {#each archives as a (a.path)}
          <li class="archive-row" class:current={inspection?.path === a.path}>
            <div class="archive-info">
              <span class="mono archive-name">{a.file_name}</span>
              <span class="archive-meta">
                {#if a.readable}
                  {a.preset_count} presets · {formatSize(a.size)} · {formatWhen(a.created_at ?? a.modified)}
                {:else}
                  Not a readable zip archive
                {/if}
              </span>
            </div>
            <button
              type="button"
              class="btn btn-ghost btn-sm"
              disabled={!a.readable || inspecting || restoring}
              onclick={() => compare(a.path)}
            >
              Compare
            </button>
          </li>
        {/each}
      </ul>
    {/if}

    {#if inspecting}
      <ProgressBar
        current={inspectProgress?.current ?? 0}
        total={inspectProgress?.total ?? 0}
        label={inspectProgress?.name ?? "Opening the archive…"}
      />
    {:else if inspection}
      <div class="compare">
        <p class="compare-title">
          <span class="mono">{fileNameOf(inspection.path)}</span> compared with what's on disk now
          {#if !inspection.has_manifest}
            <span
              class="legacy"
              title="This backup predates manifests, so its presets are matched to your locations by name. Backups made from now on record exactly where each preset came from."
              >matched by location name</span
            >
          {/if}
        </p>
        <div class="status-chips">
          {#each STATUS_ORDER as s (s)}
            {#if counts[s] > 0}
              <span class="chip status-{s}">{counts[s]} {STATUS_LABEL[s].toLowerCase()}</span>
            {/if}
          {/each}
        </div>

        {#if restorable.length === 0}
          <p class="quiet">
            Nothing to restore — every preset this backup can put back already matches what's on disk.
          </p>
        {:else}
          <div class="toolbar">
            <label class="check">
              <input
                type="checkbox"
                checked={selected.size === restorable.length}
                indeterminate={selected.size > 0 && selected.size < restorable.length}
                onchange={toggleAll}
              />
              <span>{selected.size} of {restorable.length} selected</span>
            </label>
            <label class="check">
              <input type="checkbox" bind:checked={showUnchanged} />
              <span>Show unchanged</span>
            </label>
            <button
              type="button"
              class="btn btn-primary"
              disabled={selected.size === 0 || restoring}
              onclick={() => (confirming = true)}
            >
              <ArchiveRestore size={14} /> Restore {selected.size} preset{selected.size === 1 ? "" : "s"}
            </button>
          </div>
        {/if}

        {#if groups.length > 0}
          <ul class="group-list">
            {#each groups as g (g.name)}
              {@const groupIds = g.entries.filter(isRestorable).map((e) => e.entry)}
              {@const onCount = groupIds.filter((id) => selected.has(id)).length}
              <li class="group">
                <details open={!showUnchanged}>
                  <summary>
                    {#if groupIds.length > 0}
                      <input
                        type="checkbox"
                        aria-label={`Select every changed preset in ${g.name}`}
                        checked={onCount === groupIds.length}
                        indeterminate={onCount > 0 && onCount < groupIds.length}
                        onclick={(e) => e.stopPropagation()}
                        onchange={() => toggleGroup(g.entries)}
                      />
                    {/if}
                    <span class="group-name">{g.name}</span>
                    <span class="group-count mono">{g.entries.length}</span>
                  </summary>
                  <ul class="entry-list">
                    {#each g.entries as e (e.entry)}
                      <li class="entry" class:dim={!isRestorable(e)}>
                        {#if isRestorable(e)}
                          <input
                            type="checkbox"
                            aria-label={`Restore ${e.file_name}`}
                            checked={selected.has(e.entry)}
                            onchange={() => toggle(e.entry)}
                          />
                        {:else}
                          <span class="no-check"></span>
                        {/if}
                        <span class="mono entry-name" title={e.target ?? e.rel_path}>{e.rel_path}</span>
                        <span class="chip status-{e.status}">{STATUS_LABEL[e.status]}</span>
                      </li>
                    {/each}
                  </ul>
                </details>
              </li>
            {/each}
          </ul>
        {/if}

        {#if notRestorable.length > 0}
          <details class="cannot">
            <summary class="sf-label">{notRestorable.length} can't be restored</summary>
            {#if counts.folder_gone > 0}
              <p class="quiet">
                <strong>Folder gone</strong> — the folder they lived in no longer exists, usually a
                mod that's been removed. Lineage won't recreate a mod folder MO2 didn't install.
              </p>
            {/if}
            {#if counts.unmapped > 0}
              <p class="quiet">
                <strong>Unmapped</strong> — none of your locations match where they came from{inspection
                  .unmapped_roots.length > 0
                  ? ` (${inspection.unmapped_roots.join(", ")})`
                  : ""}. Add that location in Settings to restore them.
              </p>
            {/if}
            <ul class="entry-list">
              {#each notRestorable as e (e.entry)}
                <li class="entry dim">
                  <span class="no-check"></span>
                  <span class="mono entry-name" title={e.target ?? e.rel_path}>{e.group} › {e.rel_path}</span>
                  <span class="chip status-{e.status}">{STATUS_LABEL[e.status]}</span>
                </li>
              {/each}
            </ul>
          </details>
        {/if}
      </div>
    {/if}

    {#if restoring}
      <ProgressBar
        current={restoreProgress?.current ?? 0}
        total={restoreProgress?.total ?? 0}
        label={restoreProgress?.name ?? "Snapshotting what will be overwritten…"}
      />
    {/if}

    {#if restoreOutcome && !restoring}
      <div class="note note-success success">
        <CheckCircle2 size={15} />
        <div>
          <p class="success-line">
            Restored {restoreOutcome.restored.length} preset{restoreOutcome.restored.length === 1 ? "" : "s"}.
            {#if restoreOutcome.unchanged.length > 0}
              {restoreOutcome.unchanged.length} already matched and were left alone.
            {/if}
            {#if restoreOutcome.folder_gone.length + restoreOutcome.unmapped.length > 0}
              {restoreOutcome.folder_gone.length + restoreOutcome.unmapped.length} couldn't be placed —
              their location changed after the comparison.
            {/if}
          </p>
          {#if undone}
            <p class="quiet">
              Undone — {undone.restored.length} overwritten preset{undone.restored.length === 1 ? "" : "s"} put
              back.{#if restoreOutcome.restored.length > undone.restored.length}
                {" "}Presets the restore recreated are still in place.{/if}
            </p>
          {:else if restoreOutcome.snapshot_id}
            <button type="button" class="btn btn-ghost btn-sm" disabled={undoing} onclick={undoRestore}>
              <Undo2 size={13} /> Undo
            </button>
          {/if}
        </div>
      </div>
      {#if restoreOutcome.failed.length > 0}
        <div class="note note-danger">
          {restoreOutcome.failed.length} couldn't be restored:
          <ul class="failed-list">
            {#each restoreOutcome.failed as f (f.path)}
              <li><span class="mono">{fileNameOf(f.path)}</span> — {f.reason}</li>
            {/each}
          </ul>
        </div>
      {/if}
    {/if}
  </section>
</div>

<SettingsDialog bind:open={settingsOpen} />

{#if confirming && inspection}
  <div class="confirm-scrim" role="presentation">
    <div class="confirm sf-card" role="alertdialog" aria-modal="true" aria-label="Confirm restore">
      <h2 class="sf-display">Restore {selected.size} preset{selected.size === 1 ? "" : "s"}?</h2>
      <p>
        From <strong class="mono">{fileNameOf(inspection.path)}</strong>.
        {#if selectedOverwrites > 0}
          {selectedOverwrites} will overwrite the current file — each is snapshotted first, so you can
          undo from History.
        {/if}
        {#if selectedRecreates > 0}
          {selectedRecreates} will be recreated where {selectedRecreates === 1 ? "it was" : "they were"}.
        {/if}
      </p>
      <div class="confirm-actions">
        <button type="button" class="btn btn-ghost" onclick={() => (confirming = false)}>Cancel</button>
        <button type="button" class="btn btn-primary" onclick={runRestore}>
          Restore {selected.size} preset{selected.size === 1 ? "" : "s"}
        </button>
      </div>
    </div>
  </div>
{/if}

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
  .restore {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .restore .explain {
    margin: 0;
  }
  .restore-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .restore-head h2 {
    margin: 0;
  }
  .archive-list,
  .group-list,
  .entry-list,
  .failed-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .archive-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .archive-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-md);
  }
  .archive-row.current {
    border-color: var(--sf-primary-400);
    background: var(--sf-primary-soft);
  }
  .archive-info {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .archive-name {
    font-size: 12px;
    color: var(--sf-text);
  }
  .archive-meta {
    font-size: 11px;
    color: var(--sf-text-3);
  }
  .compare {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .compare-title {
    margin: 0;
    font-size: 12.5px;
    color: var(--sf-text-2);
  }
  .legacy {
    margin-left: 6px;
    font-size: 10.5px;
    color: var(--sf-text-3);
    border-bottom: 1px dotted var(--sf-text-3);
    cursor: help;
  }
  .status-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    font-size: 10px;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    padding: 1px 7px;
    border-radius: var(--sf-r-sm);
    border: 1px solid var(--sf-line);
    color: var(--sf-text-3);
    white-space: nowrap;
  }
  .status-modified {
    color: var(--sf-warning);
    background: var(--sf-warning-soft);
    border-color: transparent;
  }
  .status-missing {
    color: var(--sf-secondary);
    background: var(--sf-secondary-soft);
    border-color: transparent;
  }
  .status-folder_gone {
    color: var(--sf-danger);
    background: var(--sf-danger-soft);
    border-color: transparent;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 18px;
  }
  .toolbar .btn-primary {
    margin-left: auto;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 12px;
    color: var(--sf-text-2);
    cursor: pointer;
  }
  .group-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .group summary {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 10px;
    border-radius: var(--sf-r-md);
    cursor: pointer;
    font-size: 12.5px;
  }
  .group summary:hover {
    background: var(--sf-hover);
  }
  .group-name {
    color: var(--sf-text);
  }
  .group-count {
    font-size: 11px;
    color: var(--sf-text-3);
  }
  .entry-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 2px 0 6px 18px;
    padding-left: 10px;
    border-left: 1px solid var(--sf-line);
  }
  .entry {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 24px;
  }
  .entry.dim .entry-name {
    color: var(--sf-text-3);
  }
  .no-check {
    width: 13px;
    flex: 0 0 auto;
  }
  .entry-name {
    flex: 1;
    min-width: 0;
    font-size: 11.5px;
    color: var(--sf-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cannot summary {
    cursor: pointer;
    padding: 2px 0;
  }
  .cannot p {
    margin: 6px 0;
  }
  .failed-list {
    margin-top: 6px;
    font-size: 12px;
  }
  .confirm-scrim {
    position: fixed;
    inset: 0;
    background: var(--sf-scrim);
    backdrop-filter: blur(6px);
    display: grid;
    place-items: center;
    z-index: 50;
  }
  .confirm {
    width: 440px;
    max-width: calc(100vw - 3rem);
    padding: 20px 22px;
    background: var(--sf-raised);
    border-color: var(--sf-border);
    box-shadow: var(--sf-e3);
  }
  .confirm h2 {
    margin: 0 0 8px;
    font-size: 16px;
  }
  .confirm p {
    margin: 0 0 18px;
    font-size: 12.5px;
    color: var(--sf-text-2);
    line-height: 1.6;
  }
  .confirm-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }
</style>
