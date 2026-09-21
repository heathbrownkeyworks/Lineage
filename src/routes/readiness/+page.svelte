<script lang="ts">
  import { ShieldCheck, ExternalLink, FolderOpen } from "lucide-svelte";
  import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
  import PageHeader from "$lib/components/PageHeader.svelte";
  import ProgressBar from "$lib/components/ProgressBar.svelte";
  import { readinessSweep } from "$lib/tauri";
  import type { ReadinessCause, ReadinessProgress, SweepReport } from "$lib/types";

  let running = $state(false);
  let progress = $state<ReadinessProgress | null>(null);
  let report = $state<SweepReport | null>(null);
  let error = $state<string | null>(null);
  /** Causes whose lists are expanded — rendered only once opened. */
  let opened = $state<Set<string>>(new Set());

  const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;
  const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
  const causeKey = (c: ReadinessCause) => `${c.status}|${c.key}`;
  const missingCauses = $derived(report?.causes.filter((c) => c.status === "missing") ?? []);
  const unconfirmedCauses = $derived(report?.causes.filter((c) => c.status === "unconfirmed") ?? []);

  async function run() {
    running = true;
    error = null;
    progress = null;
    report = null;
    opened = new Set();
    try {
      report = await readinessSweep((p) => (progress = p));
    } catch (e) {
      error = typeof e === "string" ? e : String(e);
    } finally {
      running = false;
      progress = null;
    }
  }

  function toggled(key: string, open: boolean) {
    const next = new Set(opened);
    if (open) next.add(key);
    else next.delete(key);
    opened = next;
  }

  async function openExternal(url: string) {
    try {
      await openUrl(url);
    } catch {}
  }

  async function reveal(path: string) {
    try {
      await revealItemInDir(path);
    } catch {}
  }

  function stageLabel(p: ReadinessProgress | null): string {
    if (p?.stage === "parsing") return "Reading presets…";
    if (p?.stage === "checking") return "Checking plugins, textures and archives…";
    return "Reading your setup…";
  }
</script>

{#snippet causeList(causes: ReadinessCause[])}
  <ul class="causes">
    {#each causes as c (causeKey(c))}
      <li>
        <details ontoggle={(e) => toggled(causeKey(c), (e.currentTarget as HTMLDetailsElement).open)}>
          <summary class="cause">
            <span class="title" title={c.title}>{c.title}</span>
            <span class="detail">{c.detail}</span>
            <span class="count mono">{plural(c.presets.length, "preset")}</span>
          </summary>
          {#if opened.has(causeKey(c))}
            <div class="body">
              {#if c.source_url}
                <button type="button" class="btn btn-ghost btn-sm" onclick={() => openExternal(c.source_url!)}>
                  <ExternalLink size={12} /> Open its page
                </button>
              {/if}
              <p class="sub sf-label">{plural(c.references.length, "reference")}</p>
              <ul class="mono-list">
                {#each c.references as r (r)}<li>{r}</li>{/each}
              </ul>
              <p class="sub sf-label">{plural(c.presets.length, "preset")}</p>
              <ul class="mono-list presets">
                {#each c.presets as p (p)}
                  <li>
                    <button type="button" class="preset" title={p} onclick={() => reveal(p)}>
                      <FolderOpen size={11} />{fileName(p)}
                    </button>
                  </li>
                {/each}
              </ul>
            </div>
          {/if}
        </details>
      </li>
    {/each}
  </ul>
{/snippet}

<PageHeader
  title="Readiness"
  subtitle="Which of your presets won't load as their authors made them on this setup, and why: checked against your enabled mods, active plugins and the archives that actually load."
/>

<div class="content">
  <section class="sf-card pad intro">
    <p class="quiet">
      Every preset in your JSLOT locations is read, and each plugin, texture and slider family it
      uses is looked for. Lineage only reads your setup — nothing in your profile is changed.
    </p>
    <button type="button" class="btn btn-primary" disabled={running} onclick={run}>
      <ShieldCheck size={14} />
      {report ? "Check again" : "Check all presets"}
    </button>
    {#if running}
      <div class="progress">
        <p class="stage">{stageLabel(progress)}</p>
        <ProgressBar current={progress?.current ?? 0} total={progress?.total ?? 0} label={progress?.detail ?? ""} />
      </div>
    {/if}
  </section>

  {#if error}
    <div class="note note-danger">{error}</div>
  {/if}

  {#if report && !running}
    <div class="tiles">
      <div class="tile sf-card">
        <span class="n">{report.total + report.unreadable.length}</span><span class="l">presets on {report.profile}</span>
      </div>
      <div class="tile sf-card good">
        <span class="n">{report.ready}</span><span class="l">ready</span>
      </div>
      <div class="tile sf-card bad">
        <span class="n">{report.missing}</span><span class="l">missing something</span>
      </div>
      <div class="tile sf-card" class:bad={report.unreadable.length > 0}>
        <span class="n">{report.unreadable.length}</span><span class="l">can't be read</span>
      </div>
    </div>

    {#if report.unreadable.length > 0}
      <section class="sf-card pad">
        <h2 class="sf-label">Can't be read · {report.unreadable.length}</h2>
        <p class="quiet">These files aren't valid presets. RaceMenu can't load them at all.</p>
        <ul class="mono-list">
          {#each report.unreadable as f (f.path)}
            <li>
              <button type="button" class="preset" title={f.path} onclick={() => reveal(f.path)}>
                <FolderOpen size={11} />{fileName(f.path)}
              </button>
              <span class="dim"> — {f.reason}</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if missingCauses.length > 0}
      <section class="sf-card pad">
        <h2 class="sf-label">Missing · {plural(missingCauses.length, "cause")}</h2>
        <p class="quiet">
          Most presets first — the top of this list is what fixes the most presets. A missing plugin
          means its head parts (hair, eyes, brows) don't load; a missing texture means its tint or
          overlay doesn't show.
        </p>
        {@render causeList(missingCauses)}
      </section>
    {:else}
      <div class="note note-success">
        Nothing is missing: every plugin and texture your presets use is installed and loads.
      </div>
    {/if}

    {#if unconfirmedCauses.length > 0}
      <details class="sf-card pad unconfirmed">
        <summary class="sf-label">
          Slider families not confirmed · {plural(report.unconfirmed, "preset")}
        </summary>
        <p class="quiet">
          Slider values are stored by name, and any mod can provide a family, so these aren't
          counted against readiness. A family listed here either isn't identified in the Asset
          Library, or no enabled mod comes from its Nexus page.
        </p>
        {@render causeList(unconfirmedCauses)}
      </details>
    {/if}
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
    padding: 16px 20px;
  }
  .intro {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 12px;
  }
  .progress {
    width: 100%;
  }
  .stage {
    margin: 0 0 8px;
    font-size: 12.5px;
    color: var(--sf-text-2);
  }
  .quiet {
    margin: 0 0 10px;
    color: var(--sf-text-3);
    font-size: 12.5px;
    line-height: 1.5;
  }
  .intro .quiet {
    margin: 0;
  }
  h2 {
    margin: 0 0 6px;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 12px;
  }
  .tile {
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .tile .n {
    font-family: var(--sf-font-display);
    font-size: 24px;
    color: var(--sf-text);
  }
  .tile .l {
    font-size: 12px;
    color: var(--sf-text-3);
  }
  .tile.good .n {
    color: var(--sf-success);
  }
  .tile.bad .n {
    color: var(--sf-danger);
  }
  .causes {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .cause {
    display: grid;
    grid-template-columns: minmax(0, 1.2fr) minmax(0, 2fr) auto;
    align-items: center;
    gap: 12px;
    padding: 5px 8px;
    border-radius: var(--sf-r-sm);
    cursor: pointer;
    list-style: none;
  }
  .cause::-webkit-details-marker {
    display: none;
  }
  .cause:hover {
    background: var(--sf-hover);
  }
  .title {
    font-size: 12.5px;
    color: var(--sf-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .detail {
    font-size: 11.5px;
    color: var(--sf-text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    font-size: 11px;
    color: var(--sf-secondary);
    white-space: nowrap;
  }
  .mono {
    font-family: var(--sf-font-mono);
  }
  .body {
    padding: 4px 8px 12px 20px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
  }
  .sub {
    margin: 8px 0 2px;
  }
  .mono-list {
    list-style: none;
    margin: 0;
    padding: 0;
    font-family: var(--sf-font-mono);
    font-size: 11px;
    color: var(--sf-text-2);
    max-height: 200px;
    overflow: auto;
    width: 100%;
  }
  .preset {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: none;
    border: 0;
    padding: 1px 0;
    color: var(--sf-text-2);
    font: inherit;
    cursor: pointer;
  }
  .preset:hover {
    color: var(--sf-text);
  }
  .dim {
    color: var(--sf-text-3);
  }
  .unconfirmed summary {
    cursor: pointer;
  }
  .unconfirmed .quiet {
    margin-top: 10px;
  }
</style>
