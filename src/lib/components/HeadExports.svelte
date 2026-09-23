<script lang="ts">
  import { untrack } from "svelte";
  import { FilePlus, FolderSearch, X } from "lucide-svelte";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { chooseHeadExport, formatSize, headExportsFor, headExportsInFolder } from "$lib/tauri";
  import type { ExportFile, HeadRow } from "$lib/types";

  let {
    presets,
    enabled = $bindable(true),
    rows = $bindable(new Map<string, HeadRow>()),
    disabled = false,
    onchange,
  }: {
    presets: string[];
    enabled?: boolean;
    /** Preset path → what ships for it. The page packs from this. */
    rows?: Map<string, HeadRow>;
    disabled?: boolean;
    /** Anything changed that a finished pack no longer reflects. */
    onchange?: () => void;
  } = $props();

  const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;
  const stemOf = (p: string) => fileName(p).replace(/\.[^.]*$/, "");
  const errorText = (e: unknown) => (typeof e === "string" ? e : String(e));
  const hasFiles = (r?: HeadRow) => !!r && (!!r.nif || !!r.dds);

  let finding = $state(false);
  let searching = $state(false);
  let note = $state<string | null>(null);
  let error = $state<string | null>(null);
  let rowErrors = $state<Map<string, string>>(new Map());
  let request = 0;

  // A row for every chosen preset: rows for presets no longer chosen go, and
  // new presets are looked up. Exact names only; the backend never guesses.
  $effect(() => {
    const wanted = presets;
    untrack(() => {
      const keep = new Set(wanted);
      if ([...rows.keys()].some((p) => !keep.has(p))) {
        rows = new Map([...rows].filter(([p]) => keep.has(p)));
      }
      const fresh = wanted.filter((p) => !rows.has(p));
      if (fresh.length === 0) return;
      const id = ++request;
      finding = true;
      headExportsFor(fresh)
        .then((found) => {
          if (id !== request) return;
          const next = new Map(rows);
          for (const h of found) {
            // A pick made while this ran wins.
            if (next.has(h.preset)) continue;
            next.set(h.preset, { nif: h.nif, dds: h.dds, how: h.nif || h.dds ? "found" : "none", look_in: h.look_in });
          }
          rows = next;
        })
        .catch((e) => {
          if (id === request) error = errorText(e);
        })
        .finally(() => {
          if (id === request) finding = false;
        });
    });
  });

  /** Presets without an export first, then those with one file, then complete. */
  const lines = $derived.by(() => {
    const rank = (r?: HeadRow) => (!r ? 3 : !r.nif && !r.dds ? 0 : !r.nif || !r.dds ? 1 : 2);
    return presets
      .map((p) => ({ preset: p, name: fileName(p), row: rows.get(p) }))
      .sort((a, b) => rank(a.row) - rank(b.row) || a.name.localeCompare(b.name));
  });

  const counts = $derived.by(() => {
    let both = 0;
    let one = 0;
    let none = 0;
    // A file chosen for two presets is packed once.
    const sizes = new Map<string, number>();
    for (const p of presets) {
      const r = rows.get(p);
      if (!r) continue;
      if (!r.nif && !r.dds) {
        none++;
        continue;
      }
      if (r.nif && r.dds) both++;
      else one++;
      for (const f of [r.nif, r.dds]) if (f) sizes.set(f.path.toLowerCase(), f.size);
    }
    const size = [...sizes.values()].reduce((a, b) => a + b, 0);
    return { both, one, none, size };
  });

  function describe(r: HeadRow): string {
    const files = [r.nif, r.dds].filter((f): f is ExportFile => !!f);
    const names = files.map((f) => fileName(f.path));
    const size = formatSize(files.reduce((n, f) => n + f.size, 0));
    if (files.length === 1) return `${names[0]} only · ${size}`;
    const paired = stemOf(names[0]).toLowerCase() === stemOf(names[1]).toLowerCase();
    return `${paired ? `${names[0]} + .dds` : names.join(" + ")} · ${size}`;
  }

  function setRow(preset: string, row: HeadRow) {
    const next = new Map(rows);
    next.set(preset, row);
    rows = next;
    setRowError(preset, null);
    onchange?.();
  }

  function setRowError(preset: string, message: string | null) {
    if (!message && !rowErrors.has(preset)) return;
    const next = new Map(rowErrors);
    if (message) next.set(preset, message);
    else next.delete(preset);
    rowErrors = next;
  }

  async function choose(preset: string) {
    note = null;
    const row = rows.get(preset);
    const picked = await openDialog({
      multiple: true,
      directory: false,
      defaultPath: row?.look_in || undefined,
      title: `Head export for ${fileName(preset)}`,
      filters: [{ name: "RaceMenu head export", extensions: ["nif", "dds"] }],
    });
    const files = Array.isArray(picked) ? picked : typeof picked === "string" ? [picked] : [];
    if (files.length === 0) return;
    try {
      const h = await chooseHeadExport(preset, files);
      // Only if the preset is still in the pack.
      if (presets.includes(preset)) setRow(preset, { nif: h.nif, dds: h.dds, how: "chosen", look_in: h.look_in });
    } catch (e) {
      setRowError(preset, errorText(e));
    }
  }

  function clear(preset: string) {
    setRow(preset, { nif: null, dds: null, how: "none", look_in: rows.get(preset)?.look_in ?? "" });
  }

  async function findInFolder() {
    note = null;
    error = null;
    const missing = presets.filter((p) => !hasFiles(rows.get(p)));
    if (missing.length === 0) {
      note = "Every preset already has a head export.";
      return;
    }
    const folder = await openDialog({
      directory: true,
      multiple: false,
      defaultPath: rows.get(missing[0])?.look_in || undefined,
      title: "Folder with head exports",
    });
    if (typeof folder !== "string") return;
    searching = true;
    try {
      const matched = await headExportsInFolder(folder, missing);
      const next = new Map(rows);
      let added = 0;
      for (const h of matched.found) {
        // Skip presets that left the pack, or got a pick while this ran.
        if (!presets.includes(h.preset) || hasFiles(next.get(h.preset))) continue;
        next.set(h.preset, { nif: h.nif, dds: h.dds, how: "chosen", look_in: next.get(h.preset)?.look_in ?? h.look_in });
        added++;
      }
      rows = next;
      if (added > 0) onchange?.();
      const repeats = matched.ambiguous.length;
      note =
        `Matched ${added} of ${missing.length} preset${missing.length === 1 ? "" : "s"} without one.` +
        (repeats > 0
          ? ` ${repeats} name${repeats === 1 ? " is" : "s are"} in more than one folder there; choose ${repeats === 1 ? "that one" : "those"} by hand.`
          : "");
    } catch (e) {
      error = errorText(e);
    } finally {
      searching = false;
    }
  }
</script>

<div class="heads">
  <label class="check">
    <input type="checkbox" bind:checked={enabled} {disabled} onchange={() => onchange?.()} />
    <span>
      Include RaceMenu head exports — the <span class="mono">.nif</span> and <span class="mono">.dds</span>
      from Export Head, shipped in <span class="mono">SKSE\Plugins\CharGen\</span> under each preset's name
    </span>
  </label>

  {#if enabled}
    <div class="bar">
      <p class="summary">
        {#if finding && rows.size === 0}
          Looking for head exports…
        {:else}
          {counts.both} of {presets.length} have both{counts.one ? ` · ${counts.one} only one` : ""}{counts.none
            ? ` · ${counts.none} none`
            : ""}{counts.size ? ` · ${formatSize(counts.size)}` : ""}
        {/if}
      </p>
      <button type="button" class="btn btn-ghost btn-sm" disabled={disabled || searching} onclick={findInFolder}>
        <FolderSearch size={13} />
        {searching ? "Searching…" : "Find in a folder…"}
      </button>
    </div>
    {#if note}
      <p class="quiet">{note}</p>
    {/if}
    {#if error}
      <div class="note note-danger">{error}</div>
    {/if}

    <ul class="list">
      {#each lines as line (line.preset)}
        {@const r = line.row}
        <li class="row" class:missing={r && !hasFiles(r)}>
          <span class="preset" title={line.preset}>{line.name}</span>
          <span class="files" title={[r?.nif?.path, r?.dds?.path].filter(Boolean).join("\n")}>
            {#if !r}
              looking…
            {:else if !hasFiles(r)}
              none
            {:else}
              {describe(r)}
            {/if}
          </span>
          <span class="how">{hasFiles(r) ? (r?.how === "found" ? "found" : "chosen") : ""}</span>
          <span class="actions">
            <button type="button" class="btn btn-ghost btn-sm" {disabled} onclick={() => choose(line.preset)}>
              <FilePlus size={12} />
              {hasFiles(r) ? "Change…" : "Choose…"}
            </button>
            {#if hasFiles(r)}
              <button
                type="button"
                class="icon-btn"
                aria-label={`Ship ${line.name} without a head export`}
                title="Ship without a head export"
                {disabled}
                onclick={() => clear(line.preset)}
              >
                <X size={12} />
              </button>
            {/if}
          </span>
          {#if rowErrors.get(line.preset)}
            <span class="row-error">{rowErrors.get(line.preset)}</span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .heads {
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 100%;
    min-width: 0;
  }
  .check {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    font-size: 12px;
    color: var(--sf-text-2);
    cursor: pointer;
  }
  .check input {
    margin-top: 2px;
  }
  .mono {
    font-family: var(--sf-font-mono);
  }
  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .summary {
    margin: 0;
    font-size: 12.5px;
    color: var(--sf-text-2);
  }
  .quiet {
    margin: 0;
    font-size: 12px;
    color: var(--sf-text-3);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 4px;
    max-height: 340px;
    overflow: auto;
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-md);
  }
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1.1fr) minmax(0, 1.6fr) 52px auto;
    align-items: center;
    gap: 10px;
    min-height: 30px;
    padding: 2px 8px;
    border-radius: var(--sf-r-sm);
    font-size: 12.5px;
  }
  .row:hover {
    background: var(--sf-hover);
  }
  .preset,
  .files {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .preset {
    color: var(--sf-text);
  }
  .files {
    font-family: var(--sf-font-mono);
    font-size: 11px;
    color: var(--sf-text-2);
  }
  .row.missing .files {
    color: var(--sf-warning);
  }
  .how {
    font-size: 10.5px;
    color: var(--sf-text-3);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 4px;
    justify-content: flex-end;
  }
  .icon-btn {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border: 0;
    border-radius: var(--sf-r-sm);
    background: transparent;
    color: var(--sf-text-3);
    cursor: pointer;
  }
  .icon-btn:hover:not(:disabled) {
    background: var(--sf-hover);
    color: var(--sf-text);
  }
  .row-error {
    grid-column: 1 / -1;
    font-size: 11.5px;
    color: var(--sf-danger);
    padding-bottom: 4px;
  }
</style>
