<script lang="ts">
  type Props = { current: number; total: number; label?: string };
  let { current, total, label }: Props = $props();
  const pct = $derived(total > 0 ? Math.min(100, Math.round((current / total) * 100)) : 0);
</script>

<div class="progress" role="progressbar" aria-valuenow={current} aria-valuemin={0} aria-valuemax={total} aria-label={label ?? "Progress"}>
  <div class="track">
    <div class="fill" style="width: {pct}%"></div>
  </div>
  <div class="readout">
    <span class="mono">{current} / {total}</span>
    {#if label}<span class="detail">{label}</span>{/if}
  </div>
</div>

<style>
  .track {
    height: 6px;
    border-radius: var(--sf-r-full);
    background: var(--sf-inset);
    border: 1px solid var(--sf-line);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: var(--sf-r-full);
    background: linear-gradient(90deg, var(--sf-primary-700), var(--sf-primary));
    box-shadow: var(--sf-glow);
    transition: width var(--sf-dur-fast) var(--sf-ease);
  }
  .readout {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    margin-top: 6px;
    font-size: 11px;
    color: var(--sf-text-3);
  }
  .mono {
    font-family: var(--sf-font-mono);
    color: var(--sf-secondary);
  }
  .detail {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
</style>
