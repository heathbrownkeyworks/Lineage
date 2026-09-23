<script lang="ts">
  import { untrack } from "svelte";
  import { FolderOpen, FilePlus, ListChecks, Copy, Check, X, Package, CheckCircle2 } from "lucide-svelte";
  import { goto } from "$app/navigation";
  import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import PageHeader from "$lib/components/PageHeader.svelte";
  import ProgressBar from "$lib/components/ProgressBar.svelte";
  import HeadExports from "$lib/components/HeadExports.svelte";
  import { listPresetsIn, requirementsFor, renderRequirements, packRelease } from "$lib/tauri";
  import { DEFAULT_CATEGORIES, categoryLabel } from "$lib/clean";
  import { drops } from "$lib/stores/app.svelte";
  import { isPreset } from "$lib/drop";
  import type {
    ExportFormat,
    FindProgress,
    HeadExportChoice,
    HeadRow,
    PackOutcome,
    PackProgress,
    RequirementsReport,
  } from "$lib/types";

  const FORMATS: { id: ExportFormat; label: string }[] = [
    { id: "bbcode", label: "Nexus BBCode" },
    { id: "markdown", label: "Markdown" },
    { id: "plain", label: "Plain text" },
  ];
  const cleanedAway = DEFAULT_CATEGORIES.map((c) => categoryLabel(c).toLowerCase()).join(", ");
  const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;
  /** The SKSE\Plugins\CharGen\Presets part of a path isn't a name. */
  const LAYOUT_FOLDERS = new Set(["skse", "plugins", "chargen", "presets"]);
  const errorText = (e: unknown) => (typeof e === "string" ? e : String(e));
  /** Where a link goes — enough to spot a Patreon or an Oldrim page. */
  function host(url: string): string {
    try {
      const u = new URL(url);
      const game = u.pathname.split("/")[1];
      const site = u.hostname.replace(/^www\./, "");
      return site === "nexusmods.com" && game ? `nexus · ${game}` : site;
    } catch {
      return url;
    }
  }

  let chosen = $state<string[]>([]);
  let asShipped = $state(true);
  let building = $state(false);
  let progress = $state<FindProgress | null>(null);
  let report = $state<RequirementsReport | null>(null);
  /** Requirements left out of the rendered list, by key. */
  let excluded = $state<Set<string>>(new Set());
  let format = $state<ExportFormat>("bbcode");
  let header = $state(true);
  let preview = $state("");
  let copied = $state(false);
  let error = $state<string | null>(null);
  /** The last folder picked — names the zip. */
  let pickedFolder = $state<string | null>(null);
  let subfolder = $state("");
  let headExports = $state(true);
  /** Preset → the head export that ships with it (HeadExports keeps this). */
  let heads = $state<Map<string, HeadRow>>(new Map());
  let packing = $state(false);
  let packProgress = $state<PackProgress | null>(null);
  let packed = $state<PackOutcome | null>(null);
  let packError = $state<string | null>(null);
  /** A folder's presets are being listed — the selection is about to change. */
  let listing = $state(false);
  const busy = $derived(building || packing || listing);

  const includedCount = $derived(
    report ? report.requirements.filter((r) => !excluded.has(r.key)).length : 0,
  );

  /** Any change to what's being described makes the last report stale. */
  function setChosen(paths: string[]) {
    chosen = paths;
    report = null;
    excluded = new Set();
    packed = null;
    packError = null;
  }

  function addPaths(paths: string[]) {
    const seen = new Set(chosen.map((p) => p.toLowerCase()));
    setChosen([...chosen, ...paths.filter((p) => !seen.has(p.toLowerCase()))]);
  }

  async function pickFolder() {
    error = null;
    const folder = await openDialog({ directory: true, multiple: false });
    if (typeof folder !== "string") return;
    listing = true;
    try {
      const found = await listPresetsIn(folder);
      if (found.length === 0) error = `No .jslot presets under ${folder}.`;
      else {
        addPaths(found);
        pickedFolder = folder;
      }
    } catch (e) {
      error = errorText(e);
    } finally {
      listing = false;
    }
  }

  // Presets and folders dropped on the window: presets are added as-is,
  // folders contribute every .jslot inside, like Pick a folder.
  $effect(() => {
    const paths = drops.pending;
    if (!paths) return;
    untrack(() => {
      drops.pending = null;
      void addDropped(paths);
    });
  });

  async function addDropped(paths: string[]) {
    error = null;
    if (busy) {
      error = "Busy — drop them again when this finishes.";
      return;
    }
    const found = paths.filter(isPreset);
    const others = paths.filter((p) => !isPreset(p));
    listing = true;
    try {
      for (const folder of others) {
        try {
          found.push(...(await listPresetsIn(folder)));
          pickedFolder ??= folder;
        } catch {
          // A file that isn't a preset, or an empty folder.
        }
      }
    } finally {
      listing = false;
    }
    if (found.length === 0) error = "Nothing to add: drop .jslot presets, or folders that contain them.";
    else addPaths(found);
  }

  async function pickFiles() {
    error = null;
    const picked = await openDialog({
      multiple: true,
      directory: false,
      filters: [{ name: "RaceMenu preset", extensions: ["jslot"] }],
    });
    if (Array.isArray(picked)) addPaths(picked);
    else if (typeof picked === "string") addPaths([picked]);
  }

  async function build() {
    building = true;
    error = null;
    progress = null;
    report = null;
    try {
      report = await requirementsFor(chosen, asShipped ? DEFAULT_CATEGORIES : [], (p) => (progress = p));
      excluded = new Set();
    } catch (e) {
      error = errorText(e);
    } finally {
      building = false;
      progress = null;
    }
  }

  function toggle(key: string) {
    const next = new Set(excluded);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    excluded = next;
  }

  // Re-render whenever what's included, the format or the header changes.
  // The renderer lives in Rust so the house-style rules are tested there.
  $effect(() => {
    const r = report;
    if (!r) {
      preview = "";
      return;
    }
    const lines = r.requirements
      .filter((q) => !excluded.has(q.key))
      .map((q) => ({ name: q.name, url: q.url, used_by: q.used_by }));
    const total = r.preset_count;
    const fmt = format;
    const hdr = header;
    let stale = false;
    renderRequirements(lines, total, fmt, hdr)
      .then((text) => {
        if (!stale) preview = text;
      })
      .catch((e) => (error = errorText(e)));
    return () => {
      stale = true;
    };
  });

  async function copy() {
    try {
      await navigator.clipboard.writeText(preview);
      copied = true;
      setTimeout(() => (copied = false), 1800);
    } catch (e) {
      error = `Couldn't copy: ${errorText(e)}`;
    }
  }

  /** The mod's name: the folder picked (or the first preset's), minus any
   *  SKSE\Plugins\CharGen\Presets on the end. */
  function packName(): string {
    const from = pickedFolder ?? chosen[0]?.replace(/[\\/][^\\/]*$/, "") ?? "";
    const parts = from.split(/[\\/]/).filter(Boolean);
    while (parts.length > 1 && LAYOUT_FOLDERS.has(parts[parts.length - 1].toLowerCase())) parts.pop();
    return parts[parts.length - 1] ?? "Presets";
  }

  async function savePack() {
    packError = null;
    packed = null;
    const dest = await saveDialog({
      defaultPath: `${packName()}.zip`,
      filters: [{ name: "Zip archive", extensions: ["zip"] }],
    });
    if (typeof dest !== "string") return;
    packing = true;
    packProgress = null;
    const choices: HeadExportChoice[] = headExports
      ? chosen.flatMap((preset) => {
          const r = heads.get(preset);
          return r && (r.nif || r.dds) ? [{ preset, nif: r.nif?.path ?? null, dds: r.dds?.path ?? null }] : [];
        })
      : [];
    try {
      packed = await packRelease(
        chosen,
        asShipped ? DEFAULT_CATEGORIES : [],
        subfolder,
        choices,
        dest,
        (p) => (packProgress = p),
      );
    } catch (e) {
      packError = errorText(e);
    } finally {
      packing = false;
      packProgress = null;
    }
  }

  async function showInFolder(path: string) {
    try {
      await revealItemInDir(path);
    } catch {}
  }

  function stageLabel(p: FindProgress | null): string {
    if (p?.stage === "nexus") return "Looking mods up on Nexus…";
    if (p?.stage === "resolving") return "Identifying mods…";
    return "Reading presets…";
  }
</script>

<PageHeader
  title="Release"
  subtitle="Get a preset pack ready for Nexus: the Requirements section for its page, and the zip players install."
/>

<div class="content">
  <section class="sf-card pad">
    <div class="row-head">
      <h2 class="sf-label">Presets in the pack</h2>
      <div class="pick">
        <button type="button" class="btn btn-ghost btn-sm" disabled={busy} onclick={pickFolder}>
          <FolderOpen size={13} /> Pick a folder…
        </button>
        <button type="button" class="btn btn-ghost btn-sm" disabled={busy} onclick={pickFiles}>
          <FilePlus size={13} /> Add presets…
        </button>
        {#if chosen.length > 0}
          <button
            type="button"
            class="btn btn-ghost btn-sm"
            disabled={busy}
            onclick={() => {
              setChosen([]);
              pickedFolder = null;
            }}
          >
            Clear
          </button>
        {/if}
      </div>
    </div>

    {#if chosen.length === 0}
      <p class="quiet">
        Pick the folder your pack's presets live in — every .jslot under it is included — or add
        presets one at a time.
      </p>
    {:else}
      <details class="chosen">
        <summary>{chosen.length} preset{chosen.length === 1 ? "" : "s"} chosen</summary>
        <ul class="chosen-list">
          {#each chosen as p (p)}
            <li>
              <span class="mono chosen-name" title={p}>{fileName(p)}</span>
              <button
                type="button"
                class="icon-btn"
                aria-label={`Remove ${fileName(p)}`}
                disabled={busy}
                onclick={() => setChosen(chosen.filter((c) => c !== p))}
              >
                <X size={12} />
              </button>
            </li>
          {/each}
        </ul>
      </details>
      <label class="check shipped">
        <input
          type="checkbox"
          bind:checked={asShipped}
          disabled={busy}
          onchange={() => {
            report = null;
            packed = null;
            packError = null;
          }}
        />
        <span>
          As they'll ship after Clean Preset — {cleanedAway} are removed first, in memory, so the
          list skips mods only that data used and the pack holds cleaned copies. Your preset files
          aren't changed.
        </span>
      </label>
      <button type="button" class="btn btn-primary" disabled={busy} onclick={build}>
        <ListChecks size={14} /> Build requirements
      </button>
    {/if}
  </section>

  {#if building}
    <section class="sf-card pad">
      <p class="stage">{stageLabel(progress)}</p>
      <ProgressBar
        current={progress?.current ?? 0}
        total={progress?.total ?? 0}
        label={progress?.detail ?? ""}
      />
    </section>
  {/if}

  {#if error}
    <div class="note note-danger">{error}</div>
  {/if}

  {#if report && !building}
    {@const total = report.preset_count}
    {#if report.nexus_error}
      <div class="note note-warning">
        Nexus lookups stopped early: {report.nexus_error}. Mods identified locally still appear.
      </div>
    {/if}
    {#if report.library_warning}
      <div class="note note-warning">{report.library_warning}</div>
    {/if}
    {#if !report.api_key_present}
      <p class="quiet">
        No Nexus API key is set, so names come from your library and mod folders rather than Nexus
        page titles.
      </p>
    {/if}
    {#if report.failed.length > 0}
      <details class="note note-warning">
        <summary>
          {report.failed.length} preset{report.failed.length === 1 ? "" : "s"} couldn't be read and
          {report.failed.length === 1 ? "isn't" : "aren't"} counted
        </summary>
        <ul class="mono-list">
          {#each report.failed as f (f.path)}
            <li>{fileName(f.path)} — {f.reason}</li>
          {/each}
        </ul>
      </details>
    {/if}
    {#if report.unknown.length > 0}
      <div class="note note-warning">
        <strong>
          {report.unknown.length} reference{report.unknown.length === 1 ? "" : "s"} couldn't be identified
        </strong>
        and {report.unknown.length === 1 ? "is" : "are"} left out of the list. Name them in the
        <button type="button" class="link" onclick={() => goto("/library")}>Asset Library</button>, then
        build again.
        <details class="unknown">
          <summary>Show them</summary>
          <ul class="mono-list">
            {#each report.unknown as u (u.asset.kind + u.asset.value)}
              <li>
                {u.asset.value}
                <span class="dim">· {u.asset.kind} · {u.used_by} preset{u.used_by === 1 ? "" : "s"}</span>
              </li>
            {/each}
          </ul>
        </details>
      </div>
    {/if}

    <div class="split">
      <section class="sf-card list-card">
        <div class="list-head">
          <h2 class="sf-label">
            {includedCount} of {report.requirements.length} mods · {total} preset{total === 1 ? "" : "s"}
          </h2>
        </div>
        <ul class="req-list">
          {#each report.requirements as r (r.key)}
            <li>
              <label class="req-row" class:off={excluded.has(r.key)}>
                <input type="checkbox" checked={!excluded.has(r.key)} onchange={() => toggle(r.key)} />
                <span class="req-name" title={r.name}>{r.name}</span>
                {#if r.url}
                  <span class="req-host mono" title={r.url}>{host(r.url)}</span>
                {:else}
                  <span class="req-host nolink">no link</span>
                {/if}
                <span class="req-use mono">{r.used_by === total ? "all" : `${r.used_by} / ${total}`}</span>
              </label>
            </li>
          {/each}
        </ul>
      </section>

      <section class="sf-card output">
        <div class="out-head">
          <div class="formats" role="tablist" aria-label="Output format">
            {#each FORMATS as f (f.id)}
              <button
                type="button"
                role="tab"
                class="fmt"
                class:active={format === f.id}
                aria-selected={format === f.id}
                onclick={() => (format = f.id)}
              >
                {f.label}
              </button>
            {/each}
          </div>
          <label class="check">
            <input type="checkbox" bind:checked={header} />
            <span>Section header</span>
          </label>
          <button type="button" class="btn btn-primary btn-sm copy" disabled={!preview} onclick={copy}>
            {#if copied}<Check size={13} /> Copied{:else}<Copy size={13} /> Copy{/if}
          </button>
        </div>
        <pre class="out-text">{preview}</pre>
      </section>
    </div>
  {/if}

  {#if chosen.length > 0}
    <section class="sf-card pad pack">
      <h2 class="sf-label">Pack</h2>
      <p class="quiet">
        The zip players install: presets under <span class="mono">SKSE\Plugins\CharGen\Presets\</span
        >{asShipped ? ", cleaned as above" : ", exactly as they are"}. Your files are only read.
      </p>
      <div class="pack-options">
        <label class="field">
          <span class="field-label">Folder inside Presets</span>
          <input
            class="input"
            placeholder="None — straight into Presets\"
            bind:value={subfolder}
            disabled={packing}
            oninput={() => {
              packed = null;
              packError = null;
            }}
          />
        </label>
        <HeadExports
          presets={chosen}
          bind:enabled={headExports}
          bind:rows={heads}
          disabled={busy}
          onchange={() => {
            packed = null;
            packError = null;
          }}
        />
      </div>
      <button type="button" class="btn btn-primary" disabled={busy} onclick={savePack}>
        <Package size={14} /> Save pack…
      </button>
      {#if packing}
        <div class="pack-progress">
          <ProgressBar
            current={packProgress?.current ?? 0}
            total={packProgress?.total ?? 0}
            label={packProgress?.name ?? "Starting…"}
          />
        </div>
      {/if}
      {#if packError}
        <div class="note note-danger pack-error">{packError}</div>
      {/if}
      {#if packed}
        <div class="note note-success pack-done">
          <CheckCircle2 size={15} />
          <div>
            <p>
              Packed {packed.presets} preset{packed.presets === 1 ? "" : "s"}{packed.cleaned > 0
                ? ` (${packed.cleaned} cleaned)`
                : ""}{packed.head_exports > 0
                ? ` and ${packed.head_exports} head export file${packed.head_exports === 1 ? "" : "s"}`
                : ""} into <span class="mono">{fileName(packed.path)}</span>, checked after writing.
            </p>
            <button type="button" class="btn btn-ghost btn-sm" onclick={() => showInFolder(packed!.path)}>
              <FolderOpen size={13} /> Show in folder
            </button>
          </div>
        </div>
      {/if}
    </section>
  {/if}
</div>

<style>
  .content {
    padding: 20px 28px 28px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .pad {
    padding: 18px 20px;
  }
  .row-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 10px;
  }
  .row-head h2 {
    margin: 0;
  }
  .pick {
    display: flex;
    gap: 8px;
  }
  .quiet {
    margin: 0;
    color: var(--sf-text-3);
    font-size: 12.5px;
  }
  .mono {
    font-family: var(--sf-font-mono);
  }
  .chosen summary {
    cursor: pointer;
    font-size: 12.5px;
    color: var(--sf-text-2);
  }
  .chosen-list {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
    max-height: 220px;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .chosen-list li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 1px 6px;
  }
  .chosen-name {
    font-size: 11.5px;
    color: var(--sf-text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .icon-btn {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border: 0;
    border-radius: var(--sf-r-sm);
    background: transparent;
    color: var(--sf-text-3);
    cursor: pointer;
  }
  .icon-btn:hover {
    background: var(--sf-hover);
    color: var(--sf-text);
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
  .shipped {
    margin: 14px 0;
    line-height: 1.5;
  }
  .stage {
    margin: 0 0 10px;
    font-size: 12.5px;
    color: var(--sf-text-2);
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
  .mono-list {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
    font-family: var(--sf-font-mono);
    font-size: 11px;
    max-height: 200px;
    overflow: auto;
  }
  .dim {
    color: var(--sf-text-3);
  }
  .unknown summary,
  details.note summary {
    cursor: pointer;
  }
  .split {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 14px;
    align-items: start;
  }
  .list-card,
  .output {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .list-head,
  .out-head {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--sf-line);
  }
  .list-head h2 {
    margin: 0;
  }
  .req-list {
    list-style: none;
    margin: 0;
    padding: 6px;
    max-height: 520px;
    overflow: auto;
  }
  .req-row {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 10px;
    min-height: 26px;
    padding: 2px 8px;
    border-radius: var(--sf-r-sm);
    cursor: pointer;
  }
  .req-row:hover {
    background: var(--sf-hover);
  }
  .req-row.off .req-name {
    color: var(--sf-text-3);
    text-decoration: line-through;
  }
  .req-name {
    font-size: 12.5px;
    color: var(--sf-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .req-host {
    font-size: 10.5px;
    color: var(--sf-text-3);
    white-space: nowrap;
  }
  .req-host.nolink {
    color: var(--sf-warning);
  }
  .req-use {
    font-size: 11px;
    color: var(--sf-secondary);
    white-space: nowrap;
    min-width: 64px;
    text-align: right;
  }
  .formats {
    display: flex;
    gap: 2px;
    padding: 2px;
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-md);
  }
  .fmt {
    border: 0;
    background: transparent;
    color: var(--sf-text-2);
    font-size: 11.5px;
    padding: 4px 10px;
    border-radius: var(--sf-r-sm);
    cursor: pointer;
  }
  .fmt.active {
    background: var(--sf-primary-soft);
    color: var(--sf-text);
  }
  .copy {
    margin-left: auto;
  }
  .pack {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 12px;
  }
  .pack h2 {
    margin: 0;
  }
  .pack-options {
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: 100%;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    width: 100%;
    max-width: 320px;
  }
  .field-label {
    font-size: 12px;
    color: var(--sf-text-2);
  }
  .pack-progress {
    width: 100%;
  }
  .pack-error {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    width: 100%;
    box-sizing: border-box;
  }
  .pack-done {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    width: 100%;
    box-sizing: border-box;
  }
  .pack-done p {
    margin: 0 0 8px;
  }
  .out-text {
    margin: 0;
    padding: 12px 16px;
    max-height: 520px;
    overflow: auto;
    background: var(--sf-inset);
    font-family: var(--sf-font-mono);
    font-size: 11px;
    line-height: 1.6;
    color: var(--sf-text-2);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
