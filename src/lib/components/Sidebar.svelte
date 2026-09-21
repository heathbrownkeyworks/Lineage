<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/state";
  import { goto } from "$app/navigation";
  import { Search, Archive, Eraser, Layers, BookMarked } from "lucide-svelte";
  import { backupNudge, uncoveredCount } from "$lib/stores/app.svelte";

  const items = [
    { href: "/find", label: "Find Assets", Icon: Search },
    { href: "/backup", label: "Backup", Icon: Archive },
    { href: "/remove", label: "Clean Preset", Icon: Eraser },
    { href: "/batch", label: "Batch Clean", Icon: Layers },
    { href: "/library", label: "Asset Library", Icon: BookMarked },
  ];

  let version = $state("");

  onMount(async () => {
    try {
      const { getVersion } = await import("@tauri-apps/api/app");
      version = await getVersion();
    } catch {
      // Outside the Tauri runtime (browser preview) — leave blank.
    }
  });

  /** Stays after the banner is dismissed — the quiet, persistent reminder. */
  const backupUncovered = $derived(uncoveredCount(backupNudge.status));

  function isActive(href: string): boolean {
    return page.url.pathname.startsWith(href);
  }
</script>

<aside class="sidebar">
  <nav class="nav-section" aria-label="Main navigation">
    <div class="section-label">Preset Tools</div>
    {#each items as item (item.href)}
      {@const Icon = item.Icon}
      <button
        type="button"
        class="nav-item"
        class:active={isActive(item.href)}
        aria-current={isActive(item.href) ? "page" : undefined}
        onclick={() => goto(item.href)}
      >
        <span class="nav-icon"><Icon size={15} strokeWidth={1.6} /></span>
        <span>{item.label}</span>
        {#if item.href === "/backup" && backupUncovered > 0}
          <span
            class="nav-dot"
            title={`${backupUncovered} preset${backupUncovered === 1 ? " isn't" : "s aren't"} covered by your last backup`}
          ></span>
        {/if}
      </button>
    {/each}
  </nav>

  <div class="version-line">
    <span>Lineage{#if version}&nbsp;v{version}{/if}</span>
  </div>
</aside>

<style>
  .sidebar {
    width: 196px;
    flex: 0 0 196px;
    background: var(--sf-void);
    border-right: 1px solid var(--sf-line);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .nav-section {
    padding: 12px 10px;
  }
  .section-label {
    font-weight: 600;
    font-size: 10.5px;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    white-space: nowrap;
    color: var(--sf-text-off);
    padding: 0 8px 8px;
  }
  .nav-item {
    position: relative;
    width: 100%;
    height: var(--sf-h-md);
    padding: 0 10px 0 14px;
    margin: 2px 0;
    display: flex;
    align-items: center;
    gap: 10px;
    border: 1px solid transparent;
    background: transparent;
    color: var(--sf-text-3);
    text-align: left;
    border-radius: var(--sf-r-sm);
    font-size: 13px;
    cursor: pointer;
    transition:
      background var(--sf-dur-instant) ease,
      color var(--sf-dur-instant) ease;
  }
  .nav-item:hover {
    background: var(--sf-hover);
    color: var(--sf-text);
  }
  .nav-item.active {
    color: var(--sf-primary-400);
    background: var(--sf-selected);
  }
  /* Active route indicator: a violet rule scaling in from the left. */
  .nav-item.active::before {
    content: "";
    position: absolute;
    left: 0;
    top: 7px;
    bottom: 7px;
    width: 2px;
    border-radius: 1px;
    background: linear-gradient(180deg, var(--sf-primary-400), var(--sf-primary-700));
    box-shadow: var(--sf-glow);
    transform-origin: left;
    animation: rule-in var(--sf-dur-base) var(--sf-ease) both;
  }
  @keyframes rule-in {
    from {
      transform: scaleX(0);
    }
    to {
      transform: scaleX(1);
    }
  }
  .nav-icon {
    width: 18px;
    height: 18px;
    color: inherit;
    display: inline-grid;
    place-items: center;
  }

  .version-line {
    margin: auto 12px 12px;
    padding: 6px 8px;
    border-top: 1px solid var(--sf-line);
    font-family: var(--sf-font-mono);
    font-size: 10px;
    letter-spacing: 0.1em;
    color: var(--sf-text-off);
  }
  .nav-dot {
    margin-left: auto;
    width: 7px;
    height: 7px;
    border-radius: var(--sf-r-full);
    background: var(--sf-warning);
  }
</style>
