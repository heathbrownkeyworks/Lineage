<script lang="ts">
  import { Eraser, RotateCcw, ShieldCheck, Archive } from "lucide-svelte";
  import { goto } from "$app/navigation";
  import PageHeader from "$lib/components/PageHeader.svelte";
  import PresetPicker from "$lib/components/PresetPicker.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import {
    inspectPreset,
    removeBodyMorphs,
    restoreSnapshot,
    getBackupStatus,
  } from "$lib/tauri";
  import { appEvents, bumpPresets } from "$lib/stores/app.svelte";
  import type { PresetInspection, SingleRemoveOutcome } from "$lib/types";

  let selectedPath = $state<string | null>(null);
  let inspection = $state<PresetInspection | null>(null);
  let inspecting = $state(false);
  let removing = $state(false);
  let outcome = $state<SingleRemoveOutcome | null>(null);
  let undone = $state(false);
  let undoing = $state(false);
  let error = $state<string | null>(null);
  let hasBackup = $state(true);

  // Initial load + refresh whenever settings are saved anywhere.
  $effect(() => {
    void appEvents.settingsVersion;
    getBackupStatus()
      .then((s) => (hasBackup = s.last_backup_at !== null && s.last_backup_exists))
      .catch(() => {
        // Browser preview — leave the default.
      });
  });

  async function pick(path: string) {
    selectedPath = path;
    inspection = null;
    outcome = null;
    undone = false;
    error = null;
    inspecting = true;
    try {
      inspection = await inspectPreset(path);
    } catch (e) {
      error = typeof e === "string" ? e : String(e);
    } finally {
      inspecting = false;
    }
  }

  async function remove() {
    if (!selectedPath) return;
    removing = true;
    error = null;
    try {
      outcome = await removeBodyMorphs(selectedPath);
      bumpPresets();
      // Refresh the inspection so the panel reflects the new state.
      inspection = await inspectPreset(selectedPath);
    } catch (e) {
      error = typeof e === "string" ? e : String(e);
    } finally {
      removing = false;
    }
  }

  async function undo() {
    if (!outcome) return;
    undoing = true;
    error = null;
    try {
      const report = await restoreSnapshot(outcome.snapshot_id);
      bumpPresets();
      if (report.failed.length > 0) {
        error = `Undo failed for ${report.failed[0].path}: ${report.failed[0].reason}`;
      } else {
        undone = true;
        outcome = null;
        if (selectedPath) inspection = await inspectPreset(selectedPath);
      }
    } catch (e) {
      error = typeof e === "string" ? e : String(e);
    } finally {
      undoing = false;
    }
  }
</script>

<PageHeader
  title="Remove BodySlide"
  subtitle="Preset authors often leave their own body morph data in a JSLOT. Loading such a preset silently overwrites your BodySlide setup with the author's. Removing it makes the preset apply the face without touching your body."
/>

<div class="layout">
  <div class="left">
    <PresetPicker selected={selectedPath} onpick={pick} />
  </div>

  <div class="right">
    {#if !hasBackup}
      <div class="note note-warning backup-note">
        <Archive size={14} />
        <span>
          You haven't backed up your presets yet. A single-file removal is covered by its own
          snapshot, but a full backup is the better safety net.
          <button type="button" class="link" onclick={() => goto("/backup")}>Back up first</button>
        </span>
      </div>
    {/if}

    {#if !selectedPath}
      <EmptyState
        title="Pick a preset to clean"
        body="Choose a .jslot on the left. Lineage shows you exactly what body morph data it carries before anything is changed."
      />
    {:else if inspecting}
      <p class="quiet">Reading preset…</p>
    {:else if inspection?.parse_error}
      <div class="note note-danger">
        <strong>{inspection.file_name}</strong> couldn't be read: {inspection.parse_error}
        The file was not changed.
      </div>
    {:else if inspection}
      <section class="sf-card pad">
        <h2 class="file-title mono">{inspection.file_name}</h2>
        {#if undone}
          <div class="note note-success">Restored — the preset is back to exactly how it was.</div>
        {/if}
        {#if outcome}
          <div class="note note-success result">
            <ShieldCheck size={15} />
            <div>
              <p class="m0">
                Removed <span class="mono">{outcome.detail.removed_section}</span> ({outcome.detail.removed_count}
                morph entries) from
                <span class="mono">{outcome.detail.path}</span>. Everything else is untouched.
              </p>
              <button type="button" class="btn btn-ghost btn-sm undo-btn" disabled={undoing} onclick={undo}>
                <RotateCcw size={13} /> {undoing ? "Restoring…" : "Undo"}
              </button>
            </div>
          </div>
        {:else if inspection.section === null}
          <div class="note note-info">
            This preset has no body morph data — there's nothing to remove. It already applies
            only the face.
          </div>
        {:else}
          <p class="summary">
            Contains a <span class="mono">{inspection.section}</span> section with
            <strong class="mono accent">{inspection.morph_count}</strong> morph entries.
            Removing it strips the section and leaves every other byte of the file as-is.
          </p>
          {#if !inspection.roundtrip_faithful}
            <div class="note note-warning">
              This file's formatting is unusual (probably hand-edited), so the rewrite will
              normalize its whitespace. The preset's data is preserved exactly either way.
            </div>
          {/if}
          <div class="actions">
            <button type="button" class="btn btn-danger" disabled={removing} onclick={remove}>
              <Eraser size={14} />
              {removing ? "Removing…" : `Remove ${inspection.morph_count} body morphs`}
            </button>
            <span class="safety sf-micro">A SNAPSHOT IS SAVED FIRST — UNDO ANY TIME FROM HISTORY</span>
          </div>
        {/if}
      </section>

      {#if inspection.section && inspection.section_json && !outcome}
        <section class="sf-card preview-panel">
          <div class="preview-head">
            <h3 class="sf-label">Exactly what will be removed</h3>
            <span class="preview-meta mono">{inspection.section} · {inspection.morph_count} entries</span>
          </div>
          <pre class="preview-json">{inspection.section_json}</pre>
        </section>
      {/if}
    {/if}

    {#if error}
      <div class="note note-danger">{error}</div>
    {/if}
  </div>
</div>

<style>
  .layout {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 320px 1fr;
    gap: 20px;
    padding: 20px 28px 28px;
  }
  .left {
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .left :global(.picker) {
    flex: 1;
    min-height: 0;
  }
  .right {
    min-width: 0;
    min-height: 0;
    /* Everything here is compact except the section preview, which takes the
       leftover height and scrolls internally. */
    overflow: hidden;
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding-right: 4px;
  }
  .backup-note {
    display: flex;
    gap: 10px;
    align-items: flex-start;
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--sf-secondary);
    text-decoration: underline;
    cursor: pointer;
    font-size: inherit;
  }
  .quiet {
    color: var(--sf-text-3);
    font-size: 12.5px;
  }
  .pad {
    padding: 18px 20px;
  }
  .file-title {
    margin: 0 0 12px;
    font-size: 15px;
    color: var(--sf-text);
  }
  .mono {
    font-family: var(--sf-font-mono);
  }
  .accent {
    color: var(--sf-secondary);
  }
  .summary {
    margin: 0 0 12px;
    font-size: 13px;
    color: var(--sf-text-2);
    line-height: 1.6;
  }
  /* The section preview fills the leftover height and scrolls internally. */
  .preview-panel {
    flex: 1;
    min-height: 160px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .preview-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--sf-line);
  }
  .preview-head h3 {
    margin: 0;
  }
  .preview-meta {
    font-size: 11px;
    color: var(--sf-secondary);
  }
  .preview-json {
    flex: 1;
    min-height: 0;
    margin: 0;
    padding: 12px 16px;
    overflow: auto;
    background: var(--sf-inset);
    font-family: var(--sf-font-mono);
    font-size: 11px;
    line-height: 1.6;
    color: var(--sf-text-2);
    white-space: pre;
    tab-size: 3;
  }
  .actions {
    margin-top: 18px;
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
  }
  .safety {
    letter-spacing: 0.1em;
  }
  .result {
    display: flex;
    gap: 10px;
    align-items: flex-start;
  }
  .m0 {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .undo-btn {
    margin-top: 8px;
  }
  .note {
    margin-bottom: 10px;
  }
</style>
