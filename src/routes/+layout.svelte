<script lang="ts">
  import "../app.css";
  import { onMount } from "svelte";
  import { page } from "$app/state";
  import { goto } from "$app/navigation";
  import { Download } from "lucide-svelte";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import TitleBar from "$lib/components/TitleBar.svelte";
  import BackupNudge from "$lib/components/BackupNudge.svelte";
  import { appEvents, drops, refreshBackupNudge } from "$lib/stores/app.svelte";
  import { planDrop, type DropPlan } from "$lib/drop";

  let { children } = $props();

  // Checked at launch, and again whenever settings change or anything
  // writes presets or makes a backup.
  $effect(() => {
    void appEvents.settingsVersion;
    void appEvents.presetsVersion;
    void refreshBackupNudge();
  });

  // Tauri's window drag-and-drop gives real file paths (an HTML drop only
  // gives contents). The plan says what the drop will do on this page.
  let dragging = $state<DropPlan | null>(null);
  let notice = $state<string | null>(null);
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;

  onMount(() => {
    let unlisten: (() => void) | undefined;
    let gone = false;
    import("@tauri-apps/api/webview")
      .then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent((event) => {
          const p = event.payload;
          if (p.type === "enter") dragging = planDrop(page.url.pathname, p.paths);
          else if (p.type === "leave") dragging = null;
          else if (p.type === "drop") {
            dragging = null;
            const plan = planDrop(page.url.pathname, p.paths);
            if (!plan.usable) {
              notice = plan.message;
              clearTimeout(noticeTimer);
              noticeTimer = setTimeout(() => (notice = null), 2800);
              return;
            }
            drops.pending = p.paths;
            if (plan.route !== page.url.pathname) void goto(plan.route);
          }
        }),
      )
      .then((u) => {
        if (gone) u();
        else unlisten = u;
      })
      .catch(() => {
        // Outside the Tauri runtime (browser preview): no window drops.
      });
    return () => {
      gone = true;
      unlisten?.();
      clearTimeout(noticeTimer);
    };
  });
</script>

<div class="app-shell">
  <TitleBar />
  <div class="body">
    <Sidebar />
    <main class="page">
      <BackupNudge />
      <!-- Route transition: opacity only — a translate here briefly
           overflows the scrollport and flashes a scrollbar (Visage lesson). -->
      {#key page.url.pathname}
        <div class="route-enter">
          {@render children?.()}
        </div>
      {/key}
    </main>
  </div>
  {#if dragging}
    <div class="drop-overlay" class:unusable={!dragging.usable} aria-live="polite">
      <div class="drop-card">
        <Download size={22} />
        <p>{dragging.message}</p>
      </div>
    </div>
  {/if}
  {#if notice}
    <div class="drop-notice" role="status">{notice}</div>
  {/if}
</div>

<style>
  .app-shell {
    height: 100vh;
    display: flex;
    flex-direction: column;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .page {
    flex: 1;
    overflow: auto;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    /* The content canvas sits one step above the void chrome. */
    background: var(--sf-bg);
  }
  .drop-overlay {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: grid;
    place-items: center;
    background: var(--sf-scrim);
    pointer-events: none;
  }
  .drop-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 28px 40px;
    border: 2px dashed var(--sf-primary);
    border-radius: var(--sf-r-xl);
    background: var(--sf-surface);
    color: var(--sf-text);
    font-size: 14px;
  }
  .drop-card p {
    margin: 0;
  }
  .drop-overlay.unusable .drop-card {
    border-color: var(--sf-text-3);
    color: var(--sf-text-3);
  }
  .drop-notice {
    position: fixed;
    left: 50%;
    bottom: 24px;
    transform: translateX(-50%);
    z-index: 50;
    padding: 8px 16px;
    border-radius: var(--sf-r-md);
    background: var(--sf-raised);
    border: 1px solid var(--sf-border);
    color: var(--sf-text-2);
    font-size: 12.5px;
  }
  .route-enter {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    animation: route-fade var(--sf-dur-base) var(--sf-ease) both;
  }
  @keyframes route-fade {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }
</style>
