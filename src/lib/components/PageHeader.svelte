<script lang="ts">
  type Props = { title: string; subtitle?: string };
  let { title, subtitle }: Props = $props();
</script>

<header class="page-header">
  <h1 class="sf-display">{title}</h1>
  {#if subtitle}
    <p>{subtitle}</p>
  {/if}
</header>

<style>
  .page-header {
    padding: 24px 28px 0;
  }
  h1 {
    margin: 0;
    font-size: 22px;
    color: var(--sf-text);
  }
  p {
    margin: 4px 0 0;
    font-size: 13px;
    color: var(--sf-text-3);
    max-width: 68ch;
  }
</style>
