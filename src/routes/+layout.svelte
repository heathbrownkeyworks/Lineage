<script lang="ts">
  import "../app.css";
  import { page } from "$app/state";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import TitleBar from "$lib/components/TitleBar.svelte";

  let { children } = $props();
</script>

<div class="app-shell">
  <TitleBar />
  <div class="body">
    <Sidebar />
    <main class="page">
      <!-- Route transition: opacity only — a translate here briefly
           overflows the scrollport and flashes a scrollbar (Visage lesson). -->
      {#key page.url.pathname}
        <div class="route-enter">
          {@render children?.()}
        </div>
      {/key}
    </main>
  </div>
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
