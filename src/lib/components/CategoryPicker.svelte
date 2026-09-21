<script lang="ts">
  import { CATEGORIES } from "$lib/clean";
  import type { CleanCategory } from "$lib/types";

  type Props = {
    /** The chosen categories. */
    chosen: CleanCategory[];
    /** Entries per category in what's being cleaned. A category with none
     *  is shown but can't be ticked. Omit to allow every category. */
    counts?: Partial<Record<CleanCategory, number>>;
    disabled?: boolean;
  };
  let { chosen = $bindable(), counts, disabled = false }: Props = $props();

  function toggle(id: CleanCategory) {
    chosen = chosen.includes(id) ? chosen.filter((c) => c !== id) : [...chosen, id];
  }
</script>

<fieldset class="picker" {disabled}>
  <legend class="sf-label">What to remove</legend>
  <div class="grid">
    {#each CATEGORIES as c (c.id)}
      {@const n = counts ? (counts[c.id] ?? 0) : null}
      <label class="cat" class:empty={n === 0}>
        <input
          type="checkbox"
          checked={n !== 0 && chosen.includes(c.id)}
          disabled={n === 0}
          onchange={() => toggle(c.id)}
        />
        <span class="text">
          <span class="label">
            {c.label}
            {#if n !== null}<span class="count mono">{n}</span>{/if}
          </span>
          <span class="hint">{c.hint}</span>
        </span>
      </label>
    {/each}
  </div>
  <p class="never">Face overlays — makeup, freckles, warpaint — are never touched.</p>
</fieldset>

<style>
  .picker {
    margin: 0;
    padding: 0;
    border: 0;
    min-width: 0;
  }
  .picker legend {
    padding: 0;
    margin-bottom: 8px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
    gap: 6px;
  }
  .cat {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 7px 10px;
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-md);
    cursor: pointer;
  }
  .cat:hover {
    background: var(--sf-hover);
  }
  .cat.empty {
    opacity: 0.45;
    cursor: default;
  }
  .cat.empty:hover {
    background: transparent;
  }
  .cat input {
    margin-top: 2px;
  }
  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .label {
    font-size: 12.5px;
    color: var(--sf-text);
    display: flex;
    align-items: baseline;
    gap: 6px;
  }
  .count {
    font-size: 11px;
    color: var(--sf-secondary);
  }
  .hint {
    font-size: 11px;
    color: var(--sf-text-3);
    line-height: 1.4;
  }
  .never {
    margin: 8px 0 0;
    font-size: 11px;
    color: var(--sf-text-3);
  }
</style>
