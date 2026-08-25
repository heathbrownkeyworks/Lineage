<script lang="ts">
  import type { Snippet } from "svelte";
  type Props = { title: string; body?: string; children?: Snippet };
  let { title, body, children }: Props = $props();
</script>

<div class="empty">
  <img src="/lineage-logo.svg" alt="" aria-hidden="true" />
  <h2 class="sf-display">{title}</h2>
  {#if body}
    <p>{body}</p>
  {/if}
  {#if children}
    <div class="actions">{@render children()}</div>
  {/if}
</div>

<style>
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    gap: 6px;
    padding: 56px 24px;
    border: 1px dashed var(--sf-border);
    border-radius: var(--sf-r-lg);
    background: var(--sf-surface);
  }
  img {
    width: 40px;
    height: 40px;
    opacity: 0.75;
    filter: drop-shadow(0 0 12px rgba(139, 125, 255, 0.4));
    margin-bottom: 6px;
  }
  h2 {
    margin: 0;
    font-size: 16px;
    color: var(--sf-text);
  }
  p {
    margin: 0;
    font-size: 12.5px;
    color: var(--sf-text-3);
    max-width: 52ch;
  }
  .actions {
    margin-top: 14px;
    display: flex;
    gap: 10px;
  }
</style>
