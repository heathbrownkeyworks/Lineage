<script lang="ts">
  import { ExternalLink, Globe, KeyRound, Pencil, Link as LinkIcon } from "lucide-svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import PageHeader from "$lib/components/PageHeader.svelte";
  import PresetPicker from "$lib/components/PresetPicker.svelte";
  import ProgressBar from "$lib/components/ProgressBar.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import SettingsDialog from "$lib/components/SettingsDialog.svelte";
  import LinkModal from "$lib/components/LinkModal.svelte";
  import { findAssets, getSettings } from "$lib/tauri";
  import { appEvents } from "$lib/stores/app.svelte";
  import type { AssetRef, FindAssetsReport, FindProgress, IdentifiedGroup } from "$lib/types";

  let selectedPath = $state<string | null>(null);
  let report = $state<FindAssetsReport | null>(null);
  let running = $state(false);
  let progress = $state<FindProgress | null>(null);
  let error = $state<string | null>(null);
  let settingsOpen = $state(false);
  let apiKeyPresent = $state(true);

  // Link modal — a single instance driven by whichever card/row opened it.
  let linkOpen = $state(false);
  let linkReferences = $state<{ kind: string; value: string }[]>([]);
  let linkInitialName = $state("");
  let linkInitialUrl = $state("");

  // Re-check whenever settings are saved anywhere (the gear dialog lives in
  // the TitleBar), so the "no API key" note clears as soon as a key lands.
  $effect(() => {
    void appEvents.settingsVersion;
    getSettings()
      .then((s) => (apiKeyPresent = s.nexus_api_key.trim() !== ""))
      .catch(() => {
        // Browser preview — leave the default.
      });
  });

  async function analyze(path: string) {
    selectedPath = path;
    report = null;
    error = null;
    running = true;
    progress = null;
    try {
      report = await findAssets(path, (p) => (progress = p));
    } catch (e) {
      error = typeof e === "string" ? e : String(e);
    } finally {
      running = false;
      progress = null;
    }
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

  const fileName = $derived(selectedPath?.split(/[\\/]/).pop() ?? "");
  const groupTitle = (g: IdentifiedGroup) =>
    g.name ?? g.nexus?.name ?? g.mod_folder ?? "Unknown mod";

  // Every reference in the open report — identified assets plus unknowns —
  // for the link modal's live match count.
  const contextRefs = $derived.by(() => {
    if (!report) return [];
    const fromGroups = report.identified.flatMap((g) => g.assets.map((a) => ({ kind: a.kind, value: a.value })));
    const fromUnknown = report.unknown.map((a) => ({ kind: a.kind, value: a.value }));
    return [...fromGroups, ...fromUnknown];
  });

  function openLinkForGroup(group: IdentifiedGroup) {
    linkReferences = group.assets.map((a) => ({ kind: a.kind, value: a.value }));
    linkInitialName = groupTitle(group);
    linkInitialUrl = group.page_url ?? "";
    linkOpen = true;
  }

  function openLinkForAsset(asset: AssetRef) {
    linkReferences = [{ kind: asset.kind, value: asset.value }];
    linkInitialName = "";
    linkInitialUrl = "";
    linkOpen = true;
  }

  function onLinkSaved() {
    if (selectedPath) void analyze(selectedPath);
  }

  function librarySourceLabel(source: "seed" | "user" | null): string | null {
    if (source === "user") return "from your library";
    if (source === "seed") return "built-in";
    return null;
  }
</script>

<PageHeader
  title="Find Assets"
  subtitle="Pick a preset and Lineage traces every plugin, texture, and morph it references back to the mod it came from — so you know what to download to make it work."
/>

<div class="layout">
  <div class="left">
    <PresetPicker selected={selectedPath} onpick={analyze} />
  </div>

  <div class="right">
    {#if !apiKeyPresent && !report}
      <div class="note note-info key-note">
        <KeyRound size={14} />
        <span>
          No Nexus API key is set. Lineage will still list everything a preset references,
          but can't match assets to Nexus mod pages.
          <button type="button" class="link" onclick={() => (settingsOpen = true)}>Add a key in Settings</button>
        </span>
      </div>
    {/if}

    {#if running}
      <div class="sf-card pad running">
        <p class="stage">
          {#if progress?.stage === "nexus"}
            Checking Nexus Mods…
          {:else if progress?.stage === "resolving"}
            Resolving references…
          {:else}
            Reading preset…
          {/if}
        </p>
        <ProgressBar
          current={progress?.current ?? 0}
          total={progress?.total ?? 0}
          label={progress?.detail ?? fileName}
        />
      </div>
    {:else if error}
      <div class="note note-danger">{error}</div>
    {:else if !report}
      <EmptyState
        title="Pick a preset to trace"
        body="Choose a .jslot from the list on the left, or browse to one. Lineage reads it and works out which mods its head parts, textures, and sliders come from."
      />
    {:else}
      {#if report.nexus_error}
        <div class="note note-warning">{report.nexus_error}</div>
      {/if}
      {#if report.library_warning}
        <div class="note note-warning">{report.library_warning}</div>
      {/if}

      {#if report.identified.length === 0 && report.unknown.length === 0}
        <EmptyState
          title="No external assets found"
          body="This preset only references the base game — anyone can load it without downloading mods. That's a perfectly good result."
        />
      {:else}
        {#if report.identified.length > 0}
          <section>
            <h2 class="sf-label">Identified · {report.identified.length}</h2>
            <div class="cards fx-stagger">
              {#each report.identified as group, i (group.mod_id ?? group.mod_folder ?? i)}
                <article class="mod-card sf-card" style="--i: {i}">
                  {#if group.nexus?.picture_url}
                    <img class="thumb" src={group.nexus.picture_url} alt="" loading="lazy" />
                  {:else}
                    <div class="thumb thumb-empty" aria-hidden="true">
                      <img src="/lineage-logo.svg" alt="" />
                    </div>
                  {/if}
                  <div class="mod-body">
                    <div class="card-head">
                      <h3>{groupTitle(group)}</h3>
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
                    <p class="byline">
                      {#if group.nexus?.author}by {group.nexus.author} · {/if}
                      {#if group.nexus?.category}{group.nexus.category} · {/if}
                      {#if group.nexus?.version}v{group.nexus.version} · {/if}
                      <span class="resolved mono">{group.resolved_by}</span>
                      {#if librarySourceLabel(group.library_source)}
                        <span class="kind-chip">{librarySourceLabel(group.library_source)}</span>
                      {/if}
                    </p>
                    {#if group.mod_folder && group.nexus}
                      <p class="folder mono" title={group.mod_folder}>installed as: {group.mod_folder}</p>
                    {/if}
                    <ul class="asset-chips">
                      {#each group.assets as asset (asset.kind + asset.value)}
                        <li class="chip" title={`Referenced in: ${asset.appeared_in.join(", ")}`}>
                          <span class="chip-kind">{asset.kind}</span>{asset.value}
                        </li>
                      {/each}
                    </ul>
                    {#if group.page_url}
                      <button type="button" class="btn btn-ghost btn-sm nexus-btn" onclick={() => openExternal(group.page_url!)}>
                        <ExternalLink size={13} /> {group.page_url.includes("nexusmods.com") ? "Open on Nexus" : "Open page"}
                      </button>
                    {/if}
                  </div>
                </article>
              {/each}
            </div>
          </section>
        {/if}

        {#if report.unknown.length > 0}
          <section>
            <h2 class="sf-label">Unknown · {report.unknown.length}</h2>
            <p class="section-note">
              Lineage couldn't identify where these come from — you'll need to track them down.
              Morph names usually point at slider mods; texture paths at skin or overlay packs.
            </p>
            <ul class="unknown-list">
              {#each report.unknown as asset (asset.kind + asset.value)}
                <li class="unknown-row">
                  <div class="unknown-main">
                    <span class="chip-kind">{asset.kind}</span>
                    <span class="mono val">{asset.value}</span>
                    <span class="where">in {asset.appeared_in.join(", ")}</span>
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

      {#if report.vanilla.length > 0}
        <details class="vanilla">
          <summary class="sf-label">Base game · {report.vanilla.length}</summary>
          <ul class="vanilla-list">
            {#each report.vanilla as v (v)}
              <li class="mono">{v}</li>
            {/each}
          </ul>
        </details>
      {/if}

      {#if report.rate_limit?.daily_remaining !== null && report.rate_limit}
        <p class="rate-line sf-micro">
          NEXUS API · {report.rate_limit.daily_remaining} DAILY REQUESTS REMAINING
        </p>
      {/if}
    {/if}
  </div>
</div>

<SettingsDialog bind:open={settingsOpen} />
<LinkModal
  bind:open={linkOpen}
  references={linkReferences}
  initialName={linkInitialName}
  initialUrl={linkInitialUrl}
  {contextRefs}
  onsaved={onLinkSaved}
/>

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
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 18px;
    padding-right: 4px;
  }
  .key-note {
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
  .pad {
    padding: 16px 18px;
  }
  .running .stage {
    margin: 0 0 10px;
    font-size: 13px;
    color: var(--sf-text-2);
  }
  section h2 {
    margin: 0 0 10px;
  }
  .section-note {
    margin: -4px 0 10px;
    font-size: 12px;
    color: var(--sf-text-3);
    max-width: 70ch;
  }
  .cards {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .mod-card {
    display: flex;
    gap: 14px;
    padding: 12px;
    overflow: hidden;
    transition:
      transform var(--sf-dur-fast) var(--sf-ease),
      border-color var(--sf-dur-fast) var(--sf-ease);
  }
  .mod-card:hover {
    transform: translateY(-2px);
    border-color: rgba(139, 125, 255, 0.4);
  }
  .thumb {
    width: 132px;
    height: 84px;
    flex: 0 0 132px;
    object-fit: cover;
    border-radius: var(--sf-r-md);
    border: 1px solid var(--sf-line);
    background: var(--sf-inset);
  }
  .thumb-empty {
    display: grid;
    place-items: center;
  }
  .thumb-empty img {
    width: 28px;
    height: 28px;
    opacity: 0.5;
  }
  .mod-body {
    min-width: 0;
    flex: 1;
  }
  .card-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .mod-body h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--sf-text);
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
  .byline {
    margin: 2px 0 0;
    font-size: 11.5px;
    color: var(--sf-text-3);
  }
  .kind-chip {
    display: inline-block;
    margin-left: 6px;
    font-size: 9.5px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--sf-primary-400);
    background: var(--sf-primary-soft);
    border-radius: var(--sf-r-full);
    padding: 1px 7px;
    white-space: nowrap;
    vertical-align: middle;
  }
  .resolved {
    color: var(--sf-secondary);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }
  .folder {
    margin: 2px 0 0;
    font-size: 10.5px;
    color: var(--sf-text-off);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mono {
    font-family: var(--sf-font-mono);
  }
  .asset-chips {
    list-style: none;
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin: 8px 0 0;
    padding: 0;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-family: var(--sf-font-mono);
    font-size: 10.5px;
    color: var(--sf-text-2);
    background: var(--sf-inset);
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-full);
    padding: 2px 9px;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chip-kind {
    font-size: 9px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--sf-primary-400);
  }
  .nexus-btn {
    margin-top: 10px;
  }
  .unknown-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
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
  .where {
    font-size: 10.5px;
    color: var(--sf-text-off);
  }
  .vanilla summary {
    cursor: pointer;
    padding: 4px 0;
  }
  .vanilla-list {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .vanilla-list li {
    font-size: 10.5px;
    color: var(--sf-text-3);
    background: var(--sf-inset);
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-full);
    padding: 2px 9px;
  }
  .rate-line {
    margin: 0;
    text-align: right;
  }
</style>
