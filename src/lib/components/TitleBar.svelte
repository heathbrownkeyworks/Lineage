<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { Minus, Square, Copy, X, Settings } from "lucide-svelte";
  import SettingsDialog from "./SettingsDialog.svelte";
  import { getSettings } from "$lib/tauri";

  // Resolve the Tauri window lazily and defensively — outside the Tauri
  // runtime (e.g. a plain browser preview) the globals don't exist and this
  // returns null instead of throwing, so the chrome still renders.
  function win() {
    try {
      return getCurrentWindow();
    } catch {
      return null;
    }
  }

  let isMaximized = $state(false);
  let settingsOpen = $state(false);
  let settingsWelcome = $state(false);

  async function refreshMaximized() {
    try {
      isMaximized = (await win()?.isMaximized()) ?? false;
    } catch {}
  }

  // First run: nothing works until a mod manager mode and scan roots exist,
  // so open Settings in welcome mode — once, until saved or opted out.
  async function maybeOpenFirstRunSetup() {
    try {
      const s = await getSettings();
      if (s.jslot_roots.length === 0 && !s.setup_dismissed) {
        settingsWelcome = true;
        settingsOpen = true;
      }
    } catch {
      // Outside the Tauri runtime (browser preview) — skip the prompt.
    }
  }

  onMount(() => {
    refreshMaximized();
    void maybeOpenFirstRunSetup();
    const unlistenPromise = win()?.onResized(() => refreshMaximized());
    return () => {
      unlistenPromise?.then((fn) => fn()).catch(() => {});
    };
  });

  async function onMinimize() {
    try { await win()?.minimize(); } catch {}
  }
  async function onToggleMaximize() {
    try { await win()?.toggleMaximize(); } catch {}
    refreshMaximized();
  }
  async function onClose() {
    try { await win()?.close(); } catch {}
  }
</script>

<header class="titlebar" data-tauri-drag-region>
  <div class="brand-cluster" data-tauri-drag-region>
    <img
      class="brandmark"
      src="/lineage-logo.svg"
      alt=""
      draggable="false"
      data-tauri-drag-region
      aria-hidden="true"
    />
    <div class="wordmark" data-tauri-drag-region>Lineage</div>
    <div class="tagline" data-tauri-drag-region>JSLOT Preset Utility</div>
  </div>

  <div class="window-tools" data-tauri-drag-region aria-label="Window controls">
    <button type="button" title="Settings" aria-label="Settings" onclick={() => { settingsWelcome = false; settingsOpen = true; }}>
      <Settings size={14} strokeWidth={1.6} />
    </button>
    <span class="winctl-divider" aria-hidden="true"></span>
    <button type="button" title="Minimize" aria-label="Minimize" onclick={onMinimize}>
      <Minus size={12} strokeWidth={1.6} />
    </button>
    <button
      type="button"
      title={isMaximized ? "Restore" : "Maximize"}
      aria-label={isMaximized ? "Restore" : "Maximize"}
      onclick={onToggleMaximize}
    >
      {#if isMaximized}
        <Copy size={12} strokeWidth={1.6} />
      {:else}
        <Square size={12} strokeWidth={1.6} />
      {/if}
    </button>
    <button class="close" type="button" title="Close" aria-label="Close" onclick={onClose}>
      <X size={14} strokeWidth={1.6} />
    </button>
  </div>
</header>

<SettingsDialog bind:open={settingsOpen} bind:welcome={settingsWelcome} />

<style>
  .titlebar {
    height: var(--sf-titlebar);
    flex: 0 0 var(--sf-titlebar);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 0 12px;
    /* Chrome sits below the canvas: void, with a hairline underneath. */
    background: var(--sf-void);
    border-bottom: 1px solid var(--sf-line);
    user-select: none;
    position: relative;
  }
  .brand-cluster,
  .window-tools {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .brandmark {
    width: 20px;
    height: 20px;
    display: block;
    filter: drop-shadow(0 0 8px rgba(139, 125, 255, 0.45));
    -webkit-user-drag: none;
    pointer-events: none;
  }
  .wordmark {
    font-family: var(--sf-font-display);
    font-weight: 700;
    font-size: 15px;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    background: linear-gradient(92deg, var(--sf-text), var(--sf-primary-200) 55%, var(--sf-primary));
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }
  .tagline {
    color: var(--sf-text-off);
    font-family: var(--sf-font-mono);
    font-size: 10px;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    padding-left: 8px;
    border-left: 1px solid var(--sf-line);
    margin-left: 2px;
  }
  .winctl-divider {
    width: 1px;
    height: 16px;
    margin: 0 4px;
    background: var(--sf-line);
  }
  .window-tools button {
    width: 26px;
    height: 26px;
    padding: 0;
    display: grid;
    place-items: center;
    color: var(--sf-text-2);
    border: 1px solid transparent;
    background: transparent;
    border-radius: var(--sf-r-sm);
    cursor: pointer;
    transition:
      background var(--sf-dur-instant) ease,
      color var(--sf-dur-instant) ease;
  }
  .window-tools button:hover {
    background: var(--sf-hover);
    color: var(--sf-text);
  }
  .window-tools .close:hover {
    background: var(--sf-danger-soft);
    color: var(--sf-danger);
  }
</style>
