<script lang="ts">
  import { onMount } from "svelte";
  import {
    Search,
    Plus,
    Pencil,
    Trash2,
    RotateCcw,
    ScanSearch,
    RefreshCw,
    Link as LinkIcon,
    Globe,
  } from "lucide-svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import PageHeader from "$lib/components/PageHeader.svelte";
  import ProgressBar from "$lib/components/ProgressBar.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import LinkModal from "$lib/components/LinkModal.svelte";
  import {
    libraryList,
    libraryDeleteEntry,
    libraryRestoreSeed,
    reviewCollection,
    reviewRefresh,
  } from "$lib/tauri";
  import { libraryReview } from "$lib/stores/library.svelte";
  import type { AssetRef, FindProgress, IdentifiedGroup, MergedEntry } from "$lib/types";

  // ---- Library list -------------------------------------------------------

  let entries = $state<MergedEntry[]>([]);
  let listWarning = $state<string | null>(null);
  let listLoading = $state(false);
  let listError = $state<string | null>(null);
  let search = $state("");
  let showDisabled = $state(false);

  async function loadList() {
    listLoading = true;
    listError = null;
    try {
      const listing = await libraryList();
      entries = listing.entries;
      listWarning = listing.warning;
    } catch (e) {
      listError = friendly(e);
    } finally {
      listLoading = false;
    }
  }

  onMount(() => {
    void loadList();
  });

  const totalCount = $derived(entries.length);
  const yoursCount = $derived(entries.filter((e) => e.source === "user").length);
  const builtinEntries = $derived(entries.filter((e) => e.source === "seed"));
  const builtinCount = $derived(builtinEntries.length);
  const builtinDisabledCount = $derived(builtinEntries.filter((e) => !e.enabled).length);

  const visibleEntries = $derived.by(() => {
    const q = search.trim().toLowerCase();
    return entries
      .filter((e) => showDisabled || e.enabled)
      .filter((e) => !q || e.pattern.toLowerCase().includes(q) || e.name.toLowerCase().includes(q));
  });

  async function doDelete(entry: MergedEntry) {
    listError = null;
    try {
      await libraryDeleteEntry(entry.id);
      await loadList();
    } catch (e) {
      listError = friendly(e);
    }
  }

  async function doRestore(entry: MergedEntry) {
    listError = null;
    try {
      await libraryRestoreSeed(entry.id);
      await loadList();
    } catch (e) {
      listError = friendly(e);
    }
  }

  // A single warning banner at the top covers both sources — libraryList's
  // own warning and the review report's library_warning are the same
  // underlying "library.json couldn't be read" condition.
  const topWarning = $derived(listWarning ?? libraryReview.report?.library_warning ?? null);

  // ---- Link modal — one instance, driven by whichever row/button opened it.

  let linkOpen = $state(false);
  let linkReferences = $state<{ kind: string; value: string }[]>([]);
  let linkInitialName = $state("");
  let linkInitialUrl = $state("");
  let linkInitialKind = $state<"plugin" | "texture" | "morph" | undefined>(undefined);
  let linkInitialPattern = $state<string | undefined>(undefined);
  let linkInitialMatchType = $state<"exact" | "prefix" | undefined>(undefined);
  let linkEntryId = $state<string | undefined>(undefined);

  function openAdd() {
    linkReferences = [];
    linkInitialName = "";
    linkInitialUrl = "";
    linkInitialKind = undefined;
    linkInitialPattern = undefined;
    linkInitialMatchType = undefined;
    linkEntryId = undefined;
    linkOpen = true;
  }

  function openEditEntry(entry: MergedEntry) {
    linkReferences = [];
    linkInitialName = entry.name;
    linkInitialUrl = entry.url;
    linkInitialKind = entry.kind;
    linkInitialPattern = entry.pattern;
    linkInitialMatchType = entry.match_type;
    // Only user entries update in place — editing a built-in prefills the
    // form but saves as a new user entry that shadows the seed one.
    linkEntryId = entry.source === "user" ? entry.id : undefined;
    linkOpen = true;
  }

  function openLinkForGroup(group: IdentifiedGroup) {
    linkReferences = group.assets.map((a) => ({ kind: a.kind, value: a.value }));
    linkInitialName = groupTitle(group);
    linkInitialUrl = group.page_url ?? "";
    linkInitialKind = undefined;
    linkInitialPattern = undefined;
    linkInitialMatchType = undefined;
    linkEntryId = undefined;
    linkOpen = true;
  }

  function openLinkForAsset(asset: AssetRef) {
    linkReferences = [{ kind: asset.kind, value: asset.value }];
    linkInitialName = "";
    linkInitialUrl = "";
    linkInitialKind = undefined;
    linkInitialPattern = undefined;
    linkInitialMatchType = undefined;
    linkEntryId = undefined;
    linkOpen = true;
  }

  async function onLinkSaved() {
    await loadList();
    if (libraryReview.report) await refreshReview();
  }

  // Every reference in the open report — identified assets plus unknowns —
  // for the link modal's live match count.
  const contextRefs = $derived.by(() => {
    const report = libraryReview.report;
    if (!report) return [];
    const fromGroups = report.identified.flatMap((g) => g.assets.map((a) => ({ kind: a.kind, value: a.value })));
    const fromUnknown = report.unknown.map((a) => ({ kind: a.kind, value: a.value }));
    return [...fromGroups, ...fromUnknown];
  });

  // ---- Collection review ---------------------------------------------------

  let reviewRunning = $state(false);
  let reviewProgress = $state<FindProgress | null>(null);
  let reviewError = $state<string | null>(null);
  // Plain top-level binding to the store's report, so the template's
  // {#if}/{:else} narrows it the same way find/+page.svelte narrows its own
  // (non-nested) report state.
  const reviewReport = $derived(libraryReview.report);

  async function runReview() {
    reviewRunning = true;
    reviewError = null;
    reviewProgress = null;
    try {
      libraryReview.report = await reviewCollection((p) => (reviewProgress = p));
    } catch (e) {
      reviewError = friendly(e);
    } finally {
      reviewRunning = false;
      reviewProgress = null;
    }
  }

  /** Fast path after a modal save — updates the existing report in place
   *  instead of re-parsing every preset. */
  async function refreshReview() {
    reviewError = null;
    try {
      libraryReview.report = await reviewRefresh((p) => (reviewProgress = p));
    } catch (e) {
      reviewError = friendly(e);
    } finally {
      reviewProgress = null;
    }
  }

  function presetCountKey(kind: string, value: string): string {
    return `${kind}|${value.toLowerCase()}`;
  }

  function presetCountForGroup(g: IdentifiedGroup): number {
    const report = libraryReview.report;
    if (!report) return 0;
    let max = 0;
    for (const a of g.assets) {
      const c = report.preset_counts[presetCountKey(a.kind, a.value)] ?? 0;
      if (c > max) max = c;
    }
    return max;
  }

  function presetCountForAsset(a: AssetRef): number {
    const report = libraryReview.report;
    if (!report) return 0;
    return report.preset_counts[presetCountKey(a.kind, a.value)] ?? 0;
  }

  const sortedIdentified = $derived.by(() => {
    const report = libraryReview.report;
    if (!report) return [];
    return [...report.identified].sort((a, b) => presetCountForGroup(b) - presetCountForGroup(a));
  });

  const sortedUnknown = $derived.by(() => {
    const report = libraryReview.report;
    if (!report) return [];
    return [...report.unknown].sort((a, b) => presetCountForAsset(b) - presetCountForAsset(a));
  });

  const groupTitle = (g: IdentifiedGroup) =>
    g.name ?? g.nexus?.name ?? g.mod_folder ?? "Unknown mod";

  function librarySourceLabel(source: "seed" | "user" | null): string | null {
    if (source === "user") return "from your library";
    if (source === "seed") return "built-in";
    return null;
  }

  async function openExternal(url: string) {
    try {
      await openUrl(url);
    } catch {}
  }

  function searchWeb(term: string) {
    const q = encodeURIComponent(`skyrim mod ${term}`);
    void openExternal(`https://www.google.com/search?q=${q}`);
  }

  function friendly(e: unknown): string {
    return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
  }
</script>

<PageHeader
  title="Asset Library"
  subtitle="Every mapping Lineage uses to identify presets — the links you've taught it, plus a built-in seed for well-known mods. Run a collection review to see how much of your whole preset library it already recognizes."
/>

<div class="content">
  {#if topWarning}
    <div class="note note-warning">{topWarning}</div>
  {/if}

  <section class="sf-card list-card">
    <div class="list-head">
      <div class="search">
        <Search size={13} strokeWidth={1.6} />
        <input
          class="search-input"
          type="text"
          placeholder="Filter entries…"
          bind:value={search}
          aria-label="Filter library entries"
        />
      </div>
      <div class="head-right">
        <span class="count-line">
          {totalCount} entries · {yoursCount} yours · {builtinCount} built-in ({builtinDisabledCount} disabled)
        </span>
        <label class="toggle">
          <input type="checkbox" bind:checked={showDisabled} />
          Show disabled
        </label>
        <button type="button" class="btn btn-primary btn-sm" onclick={openAdd}>
          <Plus size={13} /> Add entry
        </button>
      </div>
    </div>

    {#if listError}
      <div class="note note-danger inset-note">{listError}</div>
    {:else if listLoading && entries.length === 0}
      <p class="list-note">Loading the library…</p>
    {:else if visibleEntries.length === 0}
      <p class="list-note">
        {entries.length === 0 ? "No entries yet." : "No entries match your filter."}
      </p>
    {:else}
      <ul class="entry-list">
        {#each visibleEntries as entry (entry.id)}
          <li class="entry-row" class:disabled-row={!entry.enabled}>
            <div class="entry-main">
              <span class="kind-chip">{entry.kind}</span>
              <span class="pattern mono" title={entry.pattern}>{entry.pattern}{entry.match_type === "prefix" ? "…" : ""}</span>
              <span class="name" title={entry.name}>{entry.name}</span>
              {#if entry.url}
                <button type="button" class="link-btn mono" title={entry.url} onclick={() => openExternal(entry.url)}>
                  {entry.url}
                </button>
              {/if}
              <span class="source-badge" class:yours={entry.source === "user"}>
                {entry.source === "user" ? "Yours" : "Built-in"}
              </span>
            </div>
            <div class="entry-actions">
              <button
                type="button"
                class="icon-btn"
                title="Edit"
                aria-label={`Edit ${entry.name}`}
                onclick={() => openEditEntry(entry)}
              >
                <Pencil size={13} strokeWidth={1.6} />
              </button>
              {#if entry.source === "seed" && !entry.enabled}
                <button
                  type="button"
                  class="icon-btn"
                  title="Restore"
                  aria-label={`Restore ${entry.name}`}
                  onclick={() => doRestore(entry)}
                >
                  <RotateCcw size={13} strokeWidth={1.6} />
                </button>
              {:else}
                <button
                  type="button"
                  class="icon-btn"
                  title={entry.source === "seed" ? "Disable" : "Delete"}
                  aria-label={`${entry.source === "seed" ? "Disable" : "Delete"} ${entry.name}`}
                  onclick={() => doDelete(entry)}
                >
                  <Trash2 size={13} strokeWidth={1.6} />
                </button>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="sf-card review-card">
    <div class="review-head">
      <h2 class="sf-label">Collection Review</h2>
      {#if reviewReport && !reviewRunning}
        <button type="button" class="btn btn-ghost btn-sm" onclick={runReview}>
          <RefreshCw size={13} /> Rescan
        </button>
      {/if}
    </div>

    <div class="review-body">
      <!-- Outside the run/empty/report branches: a failed *first* run has no
           report to nest the error under, and silently dropping back to the
           empty state is what made a failure look like a hang. -->
      {#if reviewError}
        <div class="note note-danger">{reviewError}</div>
      {/if}
      {#if reviewRunning}
        <div class="stage-block">
          <p class="stage">
            {#if reviewProgress?.stage === "nexus"}
              Checking Nexus Mods…
            {:else if reviewProgress?.stage === "resolving"}
              Resolving references…
            {:else}
              Parsing presets…
            {/if}
          </p>
          <ProgressBar
            current={reviewProgress?.current ?? 0}
            total={reviewProgress?.total ?? 0}
            label={reviewProgress?.detail ?? ""}
          />
        </div>
      {:else if !reviewReport}
        <EmptyState
          title="Scan your whole collection"
          body="Lineage will read every preset across your configured locations at once, match every asset against the library, and show you what's already identified versus what still needs a link."
        >
          <button type="button" class="btn btn-primary" onclick={runReview}>
            <ScanSearch size={14} /> Review collection
          </button>
        </EmptyState>
      {:else}
        {@const report = reviewReport}
        {#if report.nexus_error}
          <div class="note note-warning">{report.nexus_error}</div>
        {/if}

        <p class="tally-line">
          <strong class="mono accent">{report.total_presets}</strong> presets scanned ·
          <strong class="mono" class:warn={report.parse_failures > 0}>{report.parse_failures}</strong> unparsable ·
          <strong class="mono">{report.identified.length}</strong> identified ·
          <strong class="mono">{report.unknown.length}</strong> unknown
        </p>

        {#if report.identified.length === 0 && report.unknown.length === 0}
          <div class="note note-info">
            Every asset across your collection is already accounted for — nothing external is
            unidentified.
          </div>
        {:else}
          {#if report.identified.length > 0}
            <section>
              <h3 class="sf-label">Identified · {sortedIdentified.length}</h3>
              <ul class="identified-list">
                {#each sortedIdentified as group (group.key)}
                  <li class="identified-row">
                    <div class="row-main">
                      <span class="name" title={groupTitle(group)}>{groupTitle(group)}</span>
                      <span class="resolved mono">{group.resolved_by}</span>
                      {#if librarySourceLabel(group.library_source)}
                        <span class="kind-chip">{librarySourceLabel(group.library_source)}</span>
                      {/if}
                    </div>
                    <div class="row-side">
                      <span class="preset-count mono">{presetCountForGroup(group)} presets</span>
                      <button
                        type="button"
                        class="icon-btn"
                        title="Edit link"
                        aria-label={`Edit link for ${groupTitle(group)}`}
                        onclick={() => openLinkForGroup(group)}
                      >
                        <Pencil size={13} strokeWidth={1.6} />
                      </button>
                    </div>
                  </li>
                {/each}
              </ul>
            </section>
          {/if}

          {#if report.unknown.length > 0}
            <section>
              <h3 class="sf-label">Unknown · {sortedUnknown.length}</h3>
              <ul class="unknown-list">
                {#each sortedUnknown as asset (asset.kind + asset.value)}
                  <li class="unknown-row">
                    <div class="unknown-main">
                      <span class="chip-kind">{asset.kind}</span>
                      <span class="mono val">{asset.value}</span>
                      <span class="preset-count mono">{presetCountForAsset(asset)} presets</span>
                    </div>
                    <div class="unknown-actions">
                      <button type="button" class="btn btn-ghost btn-sm" onclick={() => openLinkForAsset(asset)}>
                        <LinkIcon size={13} /> Add Link
                      </button>
                      <button type="button" class="btn btn-ghost btn-sm" onclick={() => searchWeb(asset.value)}>
                        <Globe size={13} /> Search the web
                      </button>
                    </div>
                  </li>
                {/each}
              </ul>
            </section>
          {/if}
        {/if}
      {/if}
    </div>
  </section>
</div>

<LinkModal
  bind:open={linkOpen}
  references={linkReferences}
  initialName={linkInitialName}
  initialUrl={linkInitialUrl}
  initialKind={linkInitialKind}
  initialPattern={linkInitialPattern}
  initialMatchType={linkInitialMatchType}
  entryId={linkEntryId}
  {contextRefs}
  onsaved={onLinkSaved}
/>

<style>
  .content {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 20px 28px 28px;
    overflow: hidden;
  }
  .mono {
    font-family: var(--sf-font-mono);
  }
  .accent {
    color: var(--sf-secondary);
  }

  /* ---- Library list ---- */
  .list-card {
    display: flex;
    flex-direction: column;
    flex: 0 0 auto;
    overflow: hidden;
  }
  .list-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px;
    flex-wrap: wrap;
    padding: 12px 16px;
    border-bottom: 1px solid var(--sf-line);
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
    width: 220px;
    flex: 0 0 auto;
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
  .head-right {
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
  }
  .count-line {
    font-size: 11.5px;
    color: var(--sf-text-3);
    white-space: nowrap;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--sf-text-2);
    cursor: pointer;
    white-space: nowrap;
  }
  .toggle input[type="checkbox"] {
    width: 13px;
    height: 13px;
    accent-color: var(--sf-primary);
  }
  .list-note {
    padding: 14px 16px;
    margin: 0;
    font-size: 12px;
    color: var(--sf-text-3);
  }
  .inset-note {
    margin: 12px 16px;
  }
  .entry-list {
    list-style: none;
    margin: 0;
    padding: 6px;
    max-height: 320px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .entry-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    min-height: var(--sf-row);
    padding: 5px 10px;
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-md);
    background: var(--sf-inset);
  }
  .entry-row.disabled-row {
    opacity: 0.55;
  }
  .entry-main {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    flex: 1;
  }
  .pattern {
    font-size: 11.5px;
    color: var(--sf-text-2);
    flex: 0 1 180px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name {
    font-size: 12.5px;
    color: var(--sf-text);
    flex: 1 1 140px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .link-btn {
    flex: 1 1 180px;
    min-width: 0;
    background: none;
    border: none;
    padding: 0;
    font-size: 11px;
    color: var(--sf-secondary);
    text-decoration: underline;
    cursor: pointer;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .source-badge {
    flex: 0 0 auto;
    font-size: 9.5px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--sf-text-off);
    background: var(--sf-surface);
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-full);
    padding: 1px 7px;
    white-space: nowrap;
  }
  .source-badge.yours {
    color: var(--sf-primary-400);
    background: var(--sf-primary-soft);
    border-color: transparent;
  }
  .entry-actions {
    display: flex;
    gap: 4px;
    flex: 0 0 auto;
  }
  .kind-chip {
    display: inline-block;
    font-size: 9.5px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--sf-primary-400);
    background: var(--sf-primary-soft);
    border-radius: var(--sf-r-full);
    padding: 1px 7px;
    white-space: nowrap;
    flex: 0 0 auto;
  }
  .chip-kind {
    font-size: 9px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--sf-primary-400);
  }
  .icon-btn {
    width: 24px;
    height: 24px;
    flex: 0 0 auto;
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

  /* ---- Collection review ---- */
  .review-card {
    flex: 1;
    min-height: 220px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .review-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--sf-line);
  }
  .review-head h2 {
    margin: 0;
  }
  .review-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .stage-block {
    padding: 4px;
  }
  .stage {
    margin: 0 0 10px;
    font-size: 13px;
    color: var(--sf-text-2);
  }
  .tally-line {
    margin: 0;
    font-size: 12.5px;
    color: var(--sf-text-2);
  }
  .tally-line .warn {
    color: var(--sf-danger);
  }
  section h3 {
    margin: 0 0 8px;
  }
  .identified-list,
  .unknown-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .identified-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    min-height: var(--sf-row);
    padding: 5px 12px;
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-md);
    background: var(--sf-surface);
  }
  .row-main {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex: 1;
    flex-wrap: wrap;
  }
  .row-main .name {
    flex: 0 1 auto;
  }
  .resolved {
    color: var(--sf-secondary);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }
  .row-side {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: 0 0 auto;
  }
  .preset-count {
    font-size: 11px;
    color: var(--sf-secondary);
    white-space: nowrap;
  }
  .unknown-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    min-height: var(--sf-row);
    padding: 5px 12px;
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-md);
    background: var(--sf-surface);
  }
  .unknown-main {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex-wrap: wrap;
  }
  .unknown-actions {
    display: flex;
    gap: 8px;
    flex: 0 0 auto;
  }
  .val {
    font-size: 11.5px;
    color: var(--sf-text);
    overflow-wrap: anywhere;
  }
</style>
