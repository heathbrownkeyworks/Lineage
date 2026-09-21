<script lang="ts">
  import { CheckCircle2, AlertTriangle, HelpCircle, ExternalLink } from "lucide-svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import type { PresetReadiness, ReadinessCheck } from "$lib/types";

  let {
    readiness,
    checking,
    error,
  }: { readiness: PresetReadiness | null; checking: boolean; error: string | null } = $props();

  const missing = $derived(readiness?.checks.filter((c) => c.status === "missing") ?? []);
  const unconfirmed = $derived(readiness?.checks.filter((c) => c.status === "unconfirmed") ?? []);
  const ready = $derived(readiness?.checks.filter((c) => c.status === "ready") ?? []);

  async function open(url: string) {
    try {
      await openUrl(url);
    } catch {}
  }
</script>

{#snippet rows(list: ReadinessCheck[])}
  <ul class="rows">
    {#each list as c (c.kind + c.value)}
      <li class="row">
        <span class="chip-kind">{c.kind}</span>
        <span class="mono val" title={c.value}>{c.value}</span>
        <span class="detail">{c.detail}</span>
        {#if c.source_url}
          <button type="button" class="btn btn-ghost btn-sm get" onclick={() => open(c.source_url!)}>
            <ExternalLink size={12} /> {c.source_name ?? "Get it"}
          </button>
        {:else if c.source_name && c.status !== "ready"}
          <span class="detail">{c.source_name}</span>
        {/if}
      </li>
    {/each}
  </ul>
{/snippet}

<section class="sf-card readiness">
  {#if checking}
    <p class="headline quiet">Checking your setup…</p>
  {:else if error}
    <p class="headline warn"><AlertTriangle size={15} /> Couldn't check this setup: {error}</p>
  {:else if readiness?.parse_error}
    <p class="headline bad"><AlertTriangle size={15} /> This preset can't be read: {readiness.parse_error}</p>
  {:else if readiness}
    {#if missing.length > 0}
      <p class="headline bad">
        <AlertTriangle size={15} />
        {missing.length} missing on {readiness.profile} — this preset won't load as its author made it.
      </p>
      {@render rows(missing)}
    {:else}
      <p class="headline good">
        <CheckCircle2 size={15} /> Ready on {readiness.profile}{readiness.checks.length === 0
          ? " — it only uses the base game."
          : " — every plugin and texture it uses is installed and loads."}
      </p>
    {/if}
    {#if unconfirmed.length > 0}
      <details class="group">
        <summary>
          <HelpCircle size={13} /> Unconfirmed · {unconfirmed.length}
          <span class="dim">— slider families no enabled mod could be matched to</span>
        </summary>
        {@render rows(unconfirmed)}
      </details>
    {/if}
    {#if ready.length > 0}
      <details class="group">
        <summary><CheckCircle2 size={13} /> Ready · {ready.length}</summary>
        {@render rows(ready)}
      </details>
    {/if}
  {/if}
</section>

<style>
  .readiness {
    padding: 14px 18px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .headline {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }
  .headline.good {
    color: var(--sf-success);
  }
  .headline.bad {
    color: var(--sf-danger);
  }
  .headline.warn {
    color: var(--sf-warning);
  }
  .quiet,
  .dim {
    color: var(--sf-text-3);
  }
  .group summary {
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--sf-text-2);
  }
  .rows {
    list-style: none;
    margin: 6px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 320px;
    overflow: auto;
  }
  .row {
    display: grid;
    grid-template-columns: 52px minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 10px;
    min-height: 26px;
    padding: 2px 6px;
    border-radius: var(--sf-r-sm);
  }
  .row:hover {
    background: var(--sf-hover);
  }
  .chip-kind {
    font-size: 9px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--sf-primary-400);
  }
  .mono {
    font-family: var(--sf-font-mono);
  }
  .val {
    font-size: 11.5px;
    color: var(--sf-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .detail {
    font-size: 11.5px;
    color: var(--sf-text-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 320px;
  }
  .get {
    white-space: nowrap;
  }
</style>
