<script lang="ts">
  import { Archive, Layers, ScanSearch, RotateCcw, ChevronDown } from "lucide-svelte";
  import { goto } from "$app/navigation";
  import PageHeader from "$lib/components/PageHeader.svelte";
  import ProgressBar from "$lib/components/ProgressBar.svelte";
  import HistoryDialog from "$lib/components/HistoryDialog.svelte";
  import {
    batchScan,
    batchRemove,
    getBackupStatus,
    formatWhen,
  } from "$lib/tauri";
  import { appEvents } from "$lib/stores/app.svelte";
  import type {
    BackupStatus,
    BatchProgress,
    BatchRemoveReport,
    BatchScanReport,
  } from "$lib/types";

  let backup = $state<BackupStatus | null>(null);
  let scanReport = $state<BatchScanReport | null>(null);
  let scanning = $state(false);
  let progress = $state<BatchProgress | null>(null);
  let selected = $state<Set<string>>(new Set());
  let confirming = $state(false);
  let running = $state(false);
  let result = $state<BatchRemoveReport | null>(null);
  let error = $state<string | null>(null);
  let failuresOpen = $state(false);
  let historyOpen = $state(false);

  const backupOk = $derived(
    backup !== null && backup.last_backup_at !== null && backup.last_backup_exists,
  );

  // Initial load + refresh whenever settings are saved anywhere.
  $effect(() => {
    void appEvents.settingsVersion;
    getBackupStatus()
      .then((s) => (backup = s))
      .catch((e) => (error = typeof e === "string" ? e : String(e)));
  });

  async function scan() {
    scanning = true;
    error = null;
    result = null;
    scanReport = null;
    progress = null;
    try {
      scanReport = await batchScan((p) => (progress = p));
      selected = new Set(scanReport.with_section.map((f) => f.path));
    } catch (e) {
      error = typeof e === "string" ? e : String(e);
    } finally {
      scanning = false;
      progress = null;
    }
  }

  function toggle(path: string) {
    const next = new Set(selected);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    selected = next;
  }

  function toggleAll() {
    if (!scanReport) return;
    selected =
      selected.size === scanReport.with_section.length
        ? new Set()
        : new Set(scanReport.with_section.map((f) => f.path));
  }

  async function confirmRun() {
    confirming = false;
    running = true;
    error = null;
    progress = null;
    try {
      result = await batchRemove([...selected], (p) => (progress = p));
      scanReport = null;
      selected = new Set();
    } catch (e) {
      error = typeof e === "string" ? e : String(e);
    } finally {
      running = false;
      progress = null;
    }
  }
</script>

<PageHeader
  title="Batch Remove"
  subtitle="Strip BodySlide data out of your whole preset collection in one pass. Every file is snapshotted before it's touched, and you choose exactly which files are included."
/>

<div class="content">
  {#if backup && !backupOk}
    <!-- Guard rail: no valid backup — batch does not proceed from here. -->
    <div class="sf-card pad guard">
      <Archive size={20} />
      <div>
        <h2>Back up before a batch run</h2>
        <p>
          {#if backup.last_backup_at && !backup.last_backup_exists}
            Your last recorded backup archive is missing from where it was saved, so Lineage
            treats this as never backed up.
          {:else}
            You haven't backed up your presets yet.
          {/if}
          A batch operation touches many files at once — the full archive is the safety net the
          per-operation snapshots build on. It takes a few seconds.
        </p>
        <button type="button" class="btn btn-primary" onclick={() => goto("/backup")}>
          <Archive size={14} /> Go to Backup
        </button>
      </div>
    </div>
  {:else if backup}
    {#if !scanReport && !result && !scanning}
      <div class="sf-card pad start">
        <p class="explain">
          Backed up {formatWhen(backup.last_backup_at ?? 0)}. Scan your locations to see which
          presets carry body morph data.
        </p>
        <button type="button" class="btn btn-primary" onclick={scan}>
          <ScanSearch size={14} /> Scan for body morphs
        </button>
      </div>
    {/if}

    {#if scanning || running}
      <div class="sf-card pad">
        <p class="stage">
          {progress?.stage === "removing" ? "Removing body morphs…" : "Scanning presets…"}
        </p>
        <ProgressBar
          current={progress?.current ?? 0}
          total={progress?.total ?? 0}
          label={progress?.name ?? ""}
        />
      </div>
    {/if}

    {#if scanReport && !running}
      <div class="tally sf-card pad">
        <div class="tally-grid">
          <div class="stat">
            <span class="big mono">{scanReport.total}</span>
            <span class="stat-label">presets found</span>
          </div>
          <div class="stat">
            <span class="big mono accent">{scanReport.with_section.length}</span>
            <span class="stat-label">with body morphs</span>
          </div>
          <div class="stat">
            <span class="big mono">{scanReport.without_section}</span>
            <span class="stat-label">already clean</span>
          </div>
          <div class="stat">
            <span class="big mono" class:warn={scanReport.failed.length > 0}>{scanReport.failed.length}</span>
            <span class="stat-label">could not parse</span>
          </div>
        </div>
        {#if scanReport.failed.length > 0}
          <details class="failed-parse">
            <summary class="sf-label">Files that could not be parsed · {scanReport.failed.length}</summary>
            <ul class="mono-list">
              {#each scanReport.failed as f (f.path)}
                <li>{f.path} — {f.reason}</li>
              {/each}
            </ul>
          </details>
        {/if}
      </div>

      {#if scanReport.with_section.length === 0}
        <div class="note note-info">
          Every preset in your collection is already clean — no body morph data anywhere.
        </div>
      {:else}
        <section class="sf-card list-card">
          <div class="list-head">
            <label class="check-all">
              <input
                type="checkbox"
                checked={selected.size === scanReport.with_section.length}
                indeterminate={selected.size > 0 && selected.size < scanReport.with_section.length}
                onchange={toggleAll}
              />
              <span>{selected.size} of {scanReport.with_section.length} selected</span>
            </label>
            <button
              type="button"
              class="btn btn-danger"
              disabled={selected.size === 0}
              onclick={() => (confirming = true)}
            >
              <Layers size={14} /> Remove from {selected.size} preset{selected.size === 1 ? "" : "s"}…
            </button>
          </div>
          <ul class="file-list">
            {#each scanReport.with_section as item (item.path)}
              <li>
                <label class="file-row">
                  <input
                    type="checkbox"
                    checked={selected.has(item.path)}
                    onchange={() => toggle(item.path)}
                  />
                  <span class="fname">{item.file_name}</span>
                  <span class="fmeta mono">{item.root_label} · {item.rel_path}</span>
                  <span class="count mono">{item.morph_count} morphs</span>
                </label>
              </li>
            {/each}
          </ul>
        </section>
      {/if}
    {/if}

    {#if result}
      <section class="sf-card pad summary">
        <h2 class="sf-label">Batch complete</h2>
        <div class="tally-grid">
          <div class="stat">
            <span class="big mono ok">{result.modified.length}</span>
            <span class="stat-label">modified</span>
          </div>
          <div class="stat">
            <span class="big mono">{result.skipped.length}</span>
            <span class="stat-label">skipped</span>
          </div>
          <div class="stat">
            <span class="big mono" class:warn={result.failed.length > 0}>{result.failed.length}</span>
            <span class="stat-label">failed</span>
          </div>
        </div>
        {#if result.failed.length > 0}
          <button type="button" class="expander" onclick={() => (failuresOpen = !failuresOpen)}>
            <ChevronDown size={13} class={failuresOpen ? "flip" : ""} />
            {failuresOpen ? "Hide" : "Show"} failure details
          </button>
          {#if failuresOpen}
            <ul class="mono-list">
              {#each result.failed as f (f.path)}
                <li>{f.path} — {f.reason}</li>
              {/each}
            </ul>
          {/if}
        {/if}
        <p class="undo-line">
          Changed your mind? The whole run can be restored from its snapshot.
          <button type="button" class="link" onclick={() => (historyOpen = true)}>
            <RotateCcw size={12} /> Open History
          </button>
        </p>
      </section>
    {/if}
  {/if}

  {#if error}
    <div class="note note-danger">{error}</div>
  {/if}
</div>

<!-- Confirmation: a real count and the backup date being relied on. -->
{#if confirming && scanReport}
  <div class="confirm-scrim" role="presentation">
    <div class="confirm sf-card" role="alertdialog" aria-modal="true" aria-label="Confirm batch removal">
      <h2 class="sf-display">Remove body morphs from {selected.size} preset{selected.size === 1 ? "" : "s"}?</h2>
      <p>
        Each file's body morph section will be removed in place. You're covered by the full
        backup from <strong>{formatWhen(backup?.last_backup_at ?? 0)}</strong> and by an
        operation snapshot taken right before anything changes.
      </p>
      <div class="confirm-actions">
        <button type="button" class="btn btn-ghost" onclick={() => (confirming = false)}>Cancel</button>
        <button type="button" class="btn btn-danger" onclick={confirmRun}>
          Remove from {selected.size} preset{selected.size === 1 ? "" : "s"}
        </button>
      </div>
    </div>
  </div>
{/if}

<HistoryDialog bind:open={historyOpen} />

<style>
  .content {
    padding: 20px 28px 28px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    flex: 1;
    min-height: 0;
  }
  .pad {
    padding: 18px 20px;
  }
  .guard {
    display: flex;
    gap: 16px;
    align-items: flex-start;
    border-color: rgba(245, 185, 92, 0.35);
    color: var(--sf-warning);
  }
  .guard h2 {
    margin: 0 0 6px;
    font-size: 15px;
    color: var(--sf-text);
  }
  .guard p {
    margin: 0 0 14px;
    font-size: 12.5px;
    color: var(--sf-text-2);
    max-width: 64ch;
    line-height: 1.55;
  }
  .start {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
  }
  .explain {
    margin: 0;
    font-size: 13px;
    color: var(--sf-text-2);
  }
  .stage {
    margin: 0 0 10px;
    font-size: 13px;
    color: var(--sf-text-2);
  }
  .tally-grid {
    display: flex;
    gap: 36px;
    flex-wrap: wrap;
  }
  .stat {
    display: flex;
    flex-direction: column;
  }
  .big {
    font-size: 24px;
    font-family: var(--sf-font-mono);
    color: var(--sf-text);
  }
  .big.accent {
    color: var(--sf-secondary);
  }
  .big.ok {
    color: var(--sf-success);
  }
  .big.warn {
    color: var(--sf-danger);
  }
  .stat-label {
    font-size: 11px;
    color: var(--sf-text-3);
    text-transform: uppercase;
    letter-spacing: 0.1em;
  }
  .mono {
    font-family: var(--sf-font-mono);
  }
  .failed-parse {
    margin-top: 12px;
  }
  .failed-parse summary {
    cursor: pointer;
  }
  .mono-list {
    margin: 8px 0 0;
    padding-left: 16px;
    font-family: var(--sf-font-mono);
    font-size: 10.5px;
    color: var(--sf-text-2);
    max-height: 160px;
    overflow-y: auto;
  }
  .list-card {
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: 1;
    overflow: hidden;
  }
  .list-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--sf-line);
  }
  .check-all {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 12.5px;
    color: var(--sf-text-2);
    cursor: pointer;
  }
  .file-list {
    list-style: none;
    margin: 0;
    padding: 6px;
    overflow-y: auto;
    flex: 1;
    min-height: 0;
  }
  .file-row {
    display: grid;
    grid-template-columns: auto minmax(120px, auto) 1fr auto;
    align-items: center;
    gap: 12px;
    min-height: var(--sf-row);
    padding: 2px 10px;
    border-radius: var(--sf-r-sm);
    cursor: pointer;
  }
  .file-row:hover {
    background: var(--sf-hover);
  }
  .fname {
    font-size: 12.5px;
    color: var(--sf-text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .fmeta {
    font-size: 10.5px;
    color: var(--sf-text-off);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    font-size: 11px;
    color: var(--sf-secondary);
    white-space: nowrap;
  }
  input[type="checkbox"] {
    width: 14px;
    height: 14px;
    accent-color: var(--sf-primary);
  }
  .summary h2 {
    margin: 0 0 12px;
  }
  .expander {
    margin-top: 12px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: none;
    border: none;
    color: var(--sf-secondary);
    font-size: 12px;
    cursor: pointer;
    padding: 2px 0;
  }
  :global(.flip) {
    transform: rotate(180deg);
  }
  .undo-line {
    margin: 14px 0 0;
    font-size: 12px;
    color: var(--sf-text-3);
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--sf-secondary);
    text-decoration: underline;
    cursor: pointer;
    font-size: inherit;
    display: inline-flex;
    align-items: center;
    gap: 4px;
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
    animation: confirm-in var(--sf-dur-base) var(--sf-ease) both;
  }
  @keyframes confirm-in {
    from {
      opacity: 0;
      transform: scale(0.97);
    }
    to {
      opacity: 1;
      transform: none;
    }
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
