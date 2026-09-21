<script lang="ts">
  import { ShieldAlert, Archive, X, CheckCircle2 } from "lucide-svelte";
  import { page } from "$app/state";
  import { runBackup, formatAgo } from "$lib/tauri";
  import { backupNudge, bumpPresets, uncoveredCount } from "$lib/stores/app.svelte";
  import type { BackupProgress } from "$lib/types";

  let running = $state(false);
  let progress = $state<BackupProgress | null>(null);
  /** Presets just backed up — shown until closed. */
  let done = $state<number | null>(null);
  let error = $state<string | null>(null);

  const status = $derived(backupNudge.status);
  const uncovered = $derived(uncoveredCount(status));
  /** The Backup page shows all of this in full, next to its own button. */
  const onBackupPage = $derived(page.url.pathname.startsWith("/backup"));
  const visible = $derived(
    !onBackupPage &&
      (done !== null || (!backupNudge.dismissed && (running || error !== null || uncovered > 0))),
  );

  const message = $derived.by(() => {
    if (!status) return "";
    const unprotected = uncovered === 1 ? "1 preset isn't protected" : `${uncovered} presets aren't protected`;
    if (status.changed_since_backup === null) {
      if (!status.last_backup_at) return `You don't have a backup yet — ${unprotected}.`;
      if (!status.last_backup_exists)
        return `Your last backup is no longer where it was saved — ${unprotected}.`;
      return `Your last backup can't be read — ${unprotected}.`;
    }
    const presets = uncovered === 1 ? "1 preset has" : `${uncovered} presets have`;
    const when = status.last_backup_at ? `, ${formatAgo(status.last_backup_at)}` : "";
    return `${presets} changed or been added since your last backup${when}.`;
  });

  async function backUp() {
    running = true;
    error = null;
    progress = null;
    try {
      const outcome = await runBackup((p) => (progress = p));
      done = outcome.file_count;
      bumpPresets();
    } catch (e) {
      error = typeof e === "string" ? e : String(e);
    } finally {
      running = false;
      progress = null;
    }
  }

  /** "Not now" holds for this session only — a nudge that could be silenced
   *  for good would stop being a safety net. */
  function notNow() {
    backupNudge.dismissed = true;
    error = null;
  }
</script>

{#if visible}
  <div class="nudge" class:ok={done !== null} role="status">
    {#if done !== null}
      <CheckCircle2 size={15} />
      <span class="msg">Backed up {done} presets.</span>
      <button type="button" class="icon-btn" aria-label="Close" onclick={() => (done = null)}>
        <X size={14} />
      </button>
    {:else}
      <ShieldAlert size={15} />
      <span class="msg">{error ?? message}</span>
      {#if running}
        <span class="mono prog">
          {progress ? `${progress.current} / ${progress.total}` : "Starting…"}
        </span>
      {:else}
        <button type="button" class="btn btn-primary btn-sm" onclick={backUp}>
          <Archive size={13} /> Back up now
        </button>
        <button type="button" class="btn btn-ghost btn-sm" onclick={notNow}>Not now</button>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .nudge {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 14px 28px 0;
    padding: 8px 12px;
    border-radius: var(--sf-r-md);
    background: var(--sf-warning-soft);
    color: var(--sf-text);
    font-size: 12.5px;
  }
  .nudge > :global(svg:first-child) {
    flex: 0 0 auto;
    color: var(--sf-warning);
  }
  .nudge.ok {
    background: var(--sf-success-soft);
  }
  .nudge.ok > :global(svg:first-child) {
    color: var(--sf-success);
  }
  .msg {
    flex: 1;
    min-width: 0;
  }
  .prog {
    font-size: 11.5px;
    color: var(--sf-text-2);
    font-family: var(--sf-font-mono);
  }
  .icon-btn {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border: 0;
    border-radius: var(--sf-r-sm);
    background: transparent;
    color: var(--sf-text-2);
    cursor: pointer;
  }
  .icon-btn:hover {
    background: var(--sf-hover);
  }
</style>
