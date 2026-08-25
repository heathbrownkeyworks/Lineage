<script lang="ts">
  import { FolderOpen, Search, RefreshCw } from "lucide-svelte";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { scanJslots } from "$lib/tauri";
  import { appEvents } from "$lib/stores/app.svelte";
  import type { JslotFile } from "$lib/types";

  type Props = {
    /** Called with the chosen preset's absolute path. */
    onpick: (path: string) => void;
    selected?: string | null;
  };
  let { onpick, selected = null }: Props = $props();

  let files = $state<JslotFile[]>([]);
  let loading = $state(false);
  let filter = $state("");
  let loadError = $state<string | null>(null);

  const filtered = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    const all = q
      ? files.filter(
          (f) =>
            f.file_name.toLowerCase().includes(q) ||
            f.rel_path.toLowerCase().includes(q) ||
            f.root_label.toLowerCase().includes(q),
        )
      : files;
    return all.slice(0, 400);
  });

  async function refresh() {
    loading = true;
    loadError = null;
    try {
      const result = await scanJslots();
      files = result.files;
    } catch (e) {
      loadError = typeof e === "string" ? e : String(e);
    } finally {
      loading = false;
    }
  }

  async function browse() {
    const picked = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "RaceMenu preset", extensions: ["jslot"] }],
    });
    if (typeof picked === "string") onpick(picked);
  }

  // Initial load + rescan whenever settings change (roots may differ).
  $effect(() => {
    void appEvents.settingsVersion;
    void refresh();
  });
</script>

<div class="picker sf-card">
  <div class="picker-head">
    <div class="search">
      <Search size={13} strokeWidth={1.6} />
      <input
        class="search-input"
        type="text"
        placeholder="Filter presets…"
        bind:value={filter}
        aria-label="Filter presets"
      />
    </div>
    <div class="picker-actions">
      <button type="button" class="btn btn-ghost btn-sm" title="Rescan locations" onclick={refresh} disabled={loading}>
        <RefreshCw size={13} /> {loading ? "Scanning…" : "Rescan"}
      </button>
      <button type="button" class="btn btn-ghost btn-sm" onclick={browse}>
        <FolderOpen size={13} /> Browse…
      </button>
    </div>
  </div>
  {#if loadError}
    <p class="picker-note">{loadError}</p>
  {:else if loading && files.length === 0}
    <p class="picker-note">Scanning your JSLOT locations…</p>
  {:else if files.length === 0}
    <p class="picker-note">
      No presets found in your configured locations — check Settings, or Browse to a file directly.
    </p>
  {:else}
    <ul class="file-list" role="listbox" aria-label="Discovered presets">
      {#each filtered as f (f.path)}
        <li>
          <button
            type="button"
            class="file-row"
            class:selected={selected === f.path}
            role="option"
            aria-selected={selected === f.path}
            onclick={() => onpick(f.path)}
          >
            <span class="fname">{f.file_name}</span>
            <span class="fmeta mono">{f.root_label} · {f.rel_path}</span>
          </button>
        </li>
      {/each}
      {#if filtered.length === 400}
        <li class="picker-note">Showing the first 400 matches — narrow the filter to see more.</li>
      {/if}
    </ul>
  {/if}
</div>

<style>
  .picker {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
  }
  .picker-head {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--sf-line);
  }
  .picker-actions {
    display: flex;
    gap: 8px;
  }
  .picker-actions .btn {
    flex: 1;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    height: var(--sf-h-sm);
    padding: 0 10px;
    border-radius: var(--sf-r-md);
    border: 1px solid var(--sf-border);
    background: var(--sf-inset);
    color: var(--sf-text-3);
  }
  .search:focus-within {
    border-color: var(--sf-primary);
  }
  .search-input {
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    outline: none;
    color: var(--sf-text);
    font-size: 12.5px;
  }
  .search-input::placeholder {
    color: var(--sf-text-off);
  }
  .picker-note {
    padding: 12px 14px;
    margin: 0;
    font-size: 12px;
    color: var(--sf-text-3);
  }
  .file-list {
    list-style: none;
    margin: 0;
    padding: 6px;
    overflow-y: auto;
    min-height: 0;
    flex: 1;
  }
  .file-row {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    padding: 5px 10px;
    border: 1px solid transparent;
    background: transparent;
    border-radius: var(--sf-r-sm);
    cursor: pointer;
    text-align: left;
    transition: background var(--sf-dur-instant) ease;
  }
  .file-row:hover {
    background: var(--sf-hover);
  }
  .file-row.selected {
    background: var(--sf-selected);
    border-color: rgba(139, 125, 255, 0.35);
  }
  .fname {
    font-size: 12.5px;
    color: var(--sf-text);
  }
  .file-row.selected .fname {
    color: var(--sf-primary-200);
  }
  .fmeta {
    font-size: 10px;
    color: var(--sf-text-off);
    font-family: var(--sf-font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }
</style>
