<script lang="ts">
  import { X, RotateCcw, RefreshCw } from "lucide-svelte";
  import { listSnapshots, restoreSnapshot, formatSize, formatWhen } from "$lib/tauri";
  import { bumpPresets } from "$lib/stores/app.svelte";
  import type { RestoreReport, SnapshotInfo } from "$lib/types";

  type Props = { open?: boolean };
  let { open: isOpen = $bindable(false) }: Props = $props();

  let dialog: HTMLDialogElement | undefined = $state();
  let snapshots = $state<SnapshotInfo[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let restoring = $state<string | null>(null);
  let results = $state<Record<string, RestoreReport>>({});

  $effect(() => {
    if (!dialog) return;
    if (isOpen) {
      error = null;
      results = {};
      void refresh();
      dialog.showModal();
    } else if (dialog.open) {
      dialog.close();
    }
  });

  async function refresh() {
    loading = true;
    try {
      snapshots = await listSnapshots();
    } catch (e) {
      error = typeof e === "string" ? e : String(e);
    } finally {
      loading = false;
    }
  }

  async function restore(id: string) {
    restoring = id;
    error = null;
    try {
      const report = await restoreSnapshot(id);
      bumpPresets();
      results = { ...results, [id]: report };
    } catch (e) {
      error = typeof e === "string" ? e : String(e);
    } finally {
      restoring = null;
    }
  }

  function operationLabel(op: string): string {
    switch (op) {
      case "remove-single":
        return "Removed BodySlide (single preset)";
      case "remove-batch":
        return "Removed BodySlide (batch)";
      case "clean-single":
        return "Cleaned preset";
      case "clean-batch":
        return "Cleaned presets (batch)";
      case "restore-backup":
        return "Restored from backup";
      default:
        return op;
    }
  }
</script>

<dialog bind:this={dialog} onclose={() => (isOpen = false)} class="history-dialog">
  <div class="frame">
    <div class="dlg-header">
      <div class="dlg-title">
        <h2 class="sf-display">History</h2>
        <span class="sf-micro">RECENT OPERATIONS · RESTORE FROM SNAPSHOTS</span>
      </div>
      <div class="head-actions">
        <button type="button" class="icon-btn" title="Refresh" aria-label="Refresh history" onclick={refresh}>
          <RefreshCw size={14} strokeWidth={1.6} />
        </button>
        <button type="button" class="icon-btn" onclick={() => (isOpen = false)} aria-label="Close history">
          <X size={15} strokeWidth={1.6} />
        </button>
      </div>
    </div>

    <div class="dlg-body">
      {#if loading}
        <p class="quiet">Loading…</p>
      {:else if snapshots.length === 0}
        <p class="quiet">
          No operations yet. When Lineage modifies presets, each operation lands here
          with a one-click restore.
        </p>
      {:else}
        <ul class="snapshots">
          {#each snapshots as snap (snap.id)}
            <li>
              <div class="row">
                <div class="snap-main">
                  <span class="op">{operationLabel(snap.operation)}</span>
                  <span class="meta">
                    {formatWhen(snap.created_at)} ·
                    {snap.entries.length} file{snap.entries.length === 1 ? "" : "s"} ·
                    {formatSize(snap.size)}
                  </span>
                </div>
                {#if snap.archive_exists}
                  <button
                    type="button"
                    class="btn btn-ghost btn-sm"
                    disabled={restoring !== null}
                    onclick={() => restore(snap.id)}
                  >
                    <RotateCcw size={13} />
                    {restoring === snap.id ? "Restoring…" : "Restore"}
                  </button>
                {:else}
                  <span class="missing">archive missing</span>
                {/if}
              </div>
              {#if results[snap.id]}
                {@const r = results[snap.id]}
                <div class="restore-result">
                  {#if r.restored.length > 0}
                    <div class="note note-success">
                      Restored {r.restored.length} file{r.restored.length === 1 ? "" : "s"} to their original locations.
                    </div>
                  {/if}
                  {#if r.missing_destination.length > 0}
                    <div class="note note-warning">
                      {r.missing_destination.length} destination{r.missing_destination.length === 1 ? "" : "s"} no longer
                      exist and were skipped (Lineage doesn't recreate deleted mod folders):
                      <ul class="mono-list">
                        {#each r.missing_destination as path (path)}<li>{path}</li>{/each}
                      </ul>
                    </div>
                  {/if}
                  {#if r.failed.length > 0}
                    <div class="note note-danger">
                      {r.failed.length} file{r.failed.length === 1 ? "" : "s"} could not be restored:
                      <ul class="mono-list">
                        {#each r.failed as f (f.path)}<li>{f.path} — {f.reason}</li>{/each}
                      </ul>
                    </div>
                  {/if}
                </div>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
      {#if error}
        <div class="note note-danger">{error}</div>
      {/if}
    </div>
  </div>
</dialog>

<style>
  .history-dialog {
    padding: 0;
    border: none;
    background: transparent;
    color: inherit;
    position: fixed;
    inset: 0;
    margin: auto;
    width: fit-content;
    height: fit-content;
    max-width: calc(100vw - 2rem);
    max-height: calc(100vh - 2rem);
    overflow: hidden;
  }
  .history-dialog::backdrop {
    background: var(--sf-scrim);
    backdrop-filter: blur(6px);
  }
  .history-dialog[open] .frame {
    animation: zoom-in var(--sf-dur-base) var(--sf-ease) both;
  }
  @keyframes zoom-in {
    from {
      opacity: 0;
      transform: scale(0.97);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  .frame {
    width: 640px;
    max-width: calc(100vw - 2rem);
    max-height: calc(100vh - 3rem);
    display: flex;
    flex-direction: column;
    background: var(--sf-raised);
    border: 1px solid var(--sf-border);
    border-radius: var(--sf-r-xl);
    box-shadow: var(--sf-e3);
    overflow: hidden;
  }
  .dlg-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--sf-line);
  }
  .dlg-title {
    display: flex;
    align-items: baseline;
    gap: 12px;
  }
  .dlg-title h2 {
    margin: 0;
    font-size: 18px;
  }
  .head-actions {
    display: flex;
    gap: 6px;
  }
  .dlg-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 16px 20px 20px;
  }
  .quiet {
    color: var(--sf-text-3);
    font-size: 13px;
  }
  .snapshots {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .snapshots li {
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-md);
    background: var(--sf-surface);
    padding: 10px 12px;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .snap-main {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .op {
    font-size: 13px;
    font-weight: 500;
    color: var(--sf-text);
  }
  .meta {
    font-size: 11.5px;
    color: var(--sf-text-3);
    font-family: var(--sf-font-mono);
  }
  .missing {
    font-size: 11.5px;
    color: var(--sf-warning);
  }
  .restore-result {
    margin-top: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .mono-list {
    margin: 6px 0 0;
    padding-left: 16px;
    font-family: var(--sf-font-mono);
    font-size: 10.5px;
    max-height: 120px;
    overflow-y: auto;
  }
  .icon-btn {
    width: 26px;
    height: 26px;
    display: grid;
    place-items: center;
    border: 1px solid transparent;
    background: transparent;
    border-radius: var(--sf-r-sm);
    color: var(--sf-text-3);
    cursor: pointer;
  }
  .icon-btn:hover {
    background: var(--sf-hover);
    color: var(--sf-text);
  }
</style>
