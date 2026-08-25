<script lang="ts">
  import {
    X,
    Check,
    FolderOpen,
    FolderPlus,
    Trash2,
    RefreshCw,
    Hand,
    Boxes,
    Tornado,
    KeyRound,
    History,
    Sparkles,
  } from "lucide-svelte";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import {
    getSettings,
    saveSettings,
    detectEnvironment,
    listMo2Profiles,
    defaultJslotRoots,
    defaultBackupDir,
    validateBackupDir,
    scanJslots,
    snapshotStats,
    clearSnapshots,
    validateNexusKey,
  } from "$lib/tauri";
  import type {
    DetectedEnvironment,
    JslotRoot,
    Mo2ProfileOption,
    ModManager,
    NexusValidation,
    ScanResult,
    Settings,
  } from "$lib/types";
  import { bumpSettings } from "$lib/stores/app.svelte";
  import HistoryDialog from "./HistoryDialog.svelte";

  type Props = { open?: boolean; welcome?: boolean };
  let { open: isOpen = $bindable(false), welcome = $bindable(false) }: Props = $props();

  let dialog: HTMLDialogElement | undefined = $state();
  let saving = $state(false);
  let saved = $state(false);
  let error = $state<string | null>(null);
  let historyOpen = $state(false);

  // Working copy of the settings.
  let modManager = $state<ModManager>("manual");
  let skyrimFolder = $state("");
  let mo2Instance = $state("");
  let mo2ModsFolder = $state("");
  let mo2ProfileDir = $state("");
  let vortexStagingFolder = $state("");
  let jslotRoots = $state<JslotRoot[]>([]);
  let backupDir = $state("");
  let snapshotRetention = $state(10);
  let savedApiKey = $state("");
  let apiKeyInput = $state("");
  let lastBackupAt = $state<number | null>(null);
  let lastBackupPath = $state<string | null>(null);
  let setupDismissed = $state(false);

  let detected = $state<DetectedEnvironment | null>(null);
  let detecting = $state(false);
  let mo2Profiles = $state<Mo2ProfileOption[]>([]);
  let scanInfo = $state<ScanResult | null>(null);
  let scanning = $state(false);
  let backupDirCheck = $state<{ writable: boolean; problem: string | null; inside_mod_tree: string | null } | null>(null);
  let snapStats = $state<{ count: number; total_size: number } | null>(null);
  let keyValidation = $state<NexusValidation | null>(null);
  let keyValidating = $state(false);
  let keyError = $state<string | null>(null);

  const managerCards: { id: ModManager; title: string; blurb: string; icon: typeof Hand }[] = [
    {
      id: "manual",
      title: "Manual",
      blurb: "No mod manager — Lineage scans your game's CharGen presets folder.",
      icon: Hand,
    },
    {
      id: "mo2",
      title: "Mod Organizer 2",
      blurb: "Scans every enabled mod in the active profile, plus the overwrite folder.",
      icon: Boxes,
    },
    {
      id: "vortex",
      title: "Vortex",
      blurb: "Scans the staging folder and the deployed Data folder.",
      icon: Tornado,
    },
  ];

  $effect(() => {
    if (!dialog) return;
    if (isOpen) {
      error = null;
      saved = false;
      void loadCurrent();
      dialog.showModal();
    } else if (dialog.open) {
      dialog.close();
    }
  });

  // Refresh the MO2 profile list whenever the mods folder changes.
  $effect(() => {
    const dir = mo2ModsFolder;
    if (!isOpen || !dir.trim()) {
      mo2Profiles = [];
      return;
    }
    let cancelled = false;
    listMo2Profiles(dir)
      .then((profiles) => {
        if (!cancelled) mo2Profiles = profiles;
      })
      .catch(() => {
        if (!cancelled) mo2Profiles = [];
      });
    return () => {
      cancelled = true;
    };
  });

  // Re-validate the backup destination when it changes.
  $effect(() => {
    const dir = backupDir;
    if (!isOpen) return;
    let cancelled = false;
    validateBackupDir(dir)
      .then((check) => {
        if (!cancelled) backupDirCheck = check;
      })
      .catch(() => {
        if (!cancelled) backupDirCheck = null;
      });
    return () => {
      cancelled = true;
    };
  });

  async function loadCurrent() {
    try {
      const s = await getSettings();
      modManager = s.mod_manager;
      skyrimFolder = s.skyrim_folder;
      mo2Instance = s.mo2_instance;
      mo2ModsFolder = s.mo2_mods_folder;
      mo2ProfileDir = s.mo2_profile_dir;
      vortexStagingFolder = s.vortex_staging_folder;
      jslotRoots = s.jslot_roots;
      backupDir = s.backup_dir;
      snapshotRetention = s.snapshot_retention;
      savedApiKey = s.nexus_api_key;
      apiKeyInput = "";
      keyValidation = null;
      keyError = null;
      lastBackupAt = s.last_backup_at;
      lastBackupPath = s.last_backup_path;
      setupDismissed = s.setup_dismissed;
      if (!backupDir.trim()) {
        backupDir = await defaultBackupDir();
      }
      void runDetect();
      void refreshSnapStats();
      void refreshScan(false);
    } catch (e) {
      error = friendly(e);
    }
  }

  /** Detection is a suggestion — it pre-fills only fields that are empty. */
  async function runDetect() {
    detecting = true;
    try {
      detected = await detectEnvironment();
      const d = detected;
      if (!skyrimFolder && d.skyrim_folder) skyrimFolder = d.skyrim_folder;
      if (!mo2Instance && d.mo2_instance) mo2Instance = d.mo2_instance;
      if (!mo2ModsFolder && d.mo2_mods_folder) mo2ModsFolder = d.mo2_mods_folder;
      if (!mo2ProfileDir && d.mo2_profile_dir) mo2ProfileDir = d.mo2_profile_dir;
      if (!vortexStagingFolder && d.vortex_staging_folder)
        vortexStagingFolder = d.vortex_staging_folder;
      // First run only: pre-select the detected manager. A saved choice sticks.
      if (welcome && jslotRoots.length === 0) {
        modManager = d.manager;
        await applyDefaultRoots();
      }
    } catch {
      detected = null;
    } finally {
      detecting = false;
    }
  }

  function currentDraft(): Settings {
    return {
      mod_manager: modManager,
      skyrim_folder: skyrimFolder,
      mo2_instance: mo2Instance,
      mo2_mods_folder: mo2ModsFolder,
      mo2_profile_dir: mo2ProfileDir,
      vortex_staging_folder: vortexStagingFolder,
      jslot_roots: jslotRoots,
      backup_dir: backupDir,
      snapshot_retention: snapshotRetention,
      nexus_api_key: apiKeyInput.trim() !== "" ? apiKeyInput.trim() : savedApiKey,
      last_backup_at: lastBackupAt,
      last_backup_path: lastBackupPath,
      setup_dismissed: setupDismissed,
    };
  }

  /** Merge the mode's default roots in: replaces stale defaults (matched by
   *  their well-known ids), keeps every root the user added by hand. */
  async function applyDefaultRoots() {
    try {
      const defaults = await defaultJslotRoots(currentDraft());
      const defaultIds = new Set(defaults.map((r) => r.id));
      jslotRoots = [...defaults, ...jslotRoots.filter((r) => !defaultIds.has(r.id))];
    } catch (e) {
      error = friendly(e);
    }
  }

  async function refreshSnapStats() {
    try {
      const s = await snapshotStats();
      snapStats = { count: s.count, total_size: s.total_size };
    } catch {
      snapStats = null;
    }
  }

  /** The live "Found N files across M locations" line. Scanning reads saved
   *  settings, so a fresh scan quietly saves the draft first. */
  async function refreshScan(saveFirst: boolean) {
    scanning = true;
    try {
      if (saveFirst) await saveSettings(currentDraft());
      scanInfo = await scanJslots();
    } catch {
      scanInfo = null;
    } finally {
      scanning = false;
    }
  }

  async function pickFolderInto(setter: (v: string) => void) {
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked === "string") setter(picked);
  }

  async function addRoot() {
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked !== "string") return;
    const label = picked.split(/[\\/]/).filter(Boolean).pop() ?? "Custom folder";
    jslotRoots = [
      ...jslotRoots,
      { id: `custom-${Date.now()}`, label, path: picked, kind: "plain" },
    ];
  }

  function removeRoot(id: string) {
    jslotRoots = jslotRoots.filter((r) => r.id !== id);
  }

  async function validateKey() {
    keyValidating = true;
    keyError = null;
    keyValidation = null;
    try {
      const key = apiKeyInput.trim() !== "" ? apiKeyInput.trim() : savedApiKey;
      keyValidation = await validateNexusKey(key);
    } catch (e) {
      keyError = friendly(e);
    } finally {
      keyValidating = false;
    }
  }

  async function clearKey() {
    savedApiKey = "";
    apiKeyInput = "";
    keyValidation = null;
    keyError = null;
  }

  async function onClearSnapshots() {
    try {
      await clearSnapshots();
      await refreshSnapStats();
    } catch (e) {
      error = friendly(e);
    }
  }

  async function save() {
    saving = true;
    error = null;
    saved = false;
    try {
      if (jslotRoots.length === 0) {
        await applyDefaultRoots();
      }
      await saveSettings(currentDraft());
      bumpSettings();
      saving = false;
      saved = true;
      setTimeout(() => {
        saved = false;
        isOpen = false;
        welcome = false;
      }, 700);
    } catch (e) {
      error = friendly(e);
      saving = false;
    }
  }

  // First-run "Don't show again": persist the dismissal and close.
  async function dismissForever() {
    setupDismissed = true;
    saving = true;
    error = null;
    try {
      await saveSettings(currentDraft());
      bumpSettings();
      isOpen = false;
      welcome = false;
    } catch (e) {
      error = friendly(e);
    } finally {
      saving = false;
    }
  }

  function close() {
    isOpen = false;
    welcome = false;
  }

  function friendly(e: unknown): string {
    return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
  }

  const maskedKey = $derived(
    savedApiKey ? `${"•".repeat(24)} (saved)` : "",
  );
</script>

<dialog
  bind:this={dialog}
  onclose={() => {
    isOpen = false;
    welcome = false;
  }}
  class="settings-dialog"
>
  <div class="frame">
    <!-- Header -->
    <div class="dlg-header">
      <div class="dlg-title">
        <h2 class="sf-display">Settings</h2>
        <span class="sf-micro">{welcome ? "FIRST-TIME SETUP" : "MOD MANAGER · LOCATIONS · BACKUPS · NEXUS"}</span>
      </div>
      <button type="button" class="icon-btn" onclick={close} aria-label="Close settings">
        <X size={15} strokeWidth={1.6} />
      </button>
    </div>

    <!-- Content -->
    <div class="dlg-body">
      {#if welcome}
        <section class="welcome sf-card">
          <img src="/lineage-logo.svg" alt="" aria-hidden="true" />
          <div>
            <h3 class="sf-display">Welcome to Lineage</h3>
            <p>
              One quick setup: pick how you manage mods and Lineage fills in the rest —
              where your <span class="mono">.jslot</span> presets live and where backups go.
              Everything here can be changed later from the gear icon.
            </p>
          </div>
        </section>
      {/if}

      <!-- Mod manager -->
      <section>
        <div class="section-head">
          <h3 class="sf-label">How do you manage mods?</h3>
          {#if detecting}
            <span class="detect-note"><RefreshCw size={11} class="spin" /> Detecting…</span>
          {:else if detected}
            <span class="detect-note">
              Detected:
              <strong>{detected.manager === "mo2" ? "Mod Organizer 2" : detected.manager === "vortex" ? "Vortex" : "Manual"}</strong>
            </span>
          {/if}
        </div>
        <div class="manager-grid">
          {#each managerCards as card (card.id)}
            {@const selected = modManager === card.id}
            {@const CardIcon = card.icon}
            <button
              type="button"
              class="manager-card"
              class:selected
              onclick={async () => {
                modManager = card.id;
                await applyDefaultRoots();
              }}
            >
              {#if detected?.manager === card.id}
                <span class="detected-chip">Detected</span>
              {/if}
              <CardIcon size={18} strokeWidth={1.6} />
              <div class="card-title">{card.title}</div>
              <p>{card.blurb}</p>
            </button>
          {/each}
        </div>
      </section>

      <!-- Mode-specific folders -->
      <section class="sf-card pad">
        <h3 class="sf-label">Folders</h3>
        <div class="fields">
          {#if modManager === "manual" || modManager === "vortex"}
            <div class="field">
              <label for="skyrim-folder">Skyrim install folder</label>
              <div class="path-row">
                <input
                  id="skyrim-folder"
                  class="input input-mono"
                  type="text"
                  bind:value={skyrimFolder}
                  placeholder="D:\Steam\steamapps\common\Skyrim Special Edition"
                />
                <button type="button" class="btn btn-ghost btn-sm" onclick={() => pickFolderInto((v) => (skyrimFolder = v))}>
                  <FolderOpen size={13} /> Browse
                </button>
              </div>
            </div>
          {/if}
          {#if modManager === "mo2"}
            <div class="field">
              <label for="mo2-instance">MO2 instance folder <span class="hint">(contains ModOrganizer.ini)</span></label>
              <div class="path-row">
                <input
                  id="mo2-instance"
                  class="input input-mono"
                  type="text"
                  bind:value={mo2Instance}
                  placeholder="D:\Nordic Souls"
                />
                <button type="button" class="btn btn-ghost btn-sm" onclick={() => pickFolderInto((v) => (mo2Instance = v))}>
                  <FolderOpen size={13} /> Browse
                </button>
              </div>
            </div>
            <div class="field">
              <label for="mo2-mods">MO2 mods folder</label>
              <div class="path-row">
                <input
                  id="mo2-mods"
                  class="input input-mono"
                  type="text"
                  bind:value={mo2ModsFolder}
                  placeholder="D:\Nordic Souls\mods"
                />
                <button type="button" class="btn btn-ghost btn-sm" onclick={() => pickFolderInto((v) => (mo2ModsFolder = v))}>
                  <FolderOpen size={13} /> Browse
                </button>
              </div>
            </div>
            <div class="field">
              <label for="mo2-profile">Active profile</label>
              {#if mo2Profiles.length > 0}
                <select id="mo2-profile" class="input" bind:value={mo2ProfileDir}>
                  <option value="" disabled>Pick a profile…</option>
                  {#each mo2Profiles as p (p.path)}
                    <option value={p.path}>{p.name}</option>
                  {/each}
                </select>
              {:else}
                <div class="path-row">
                  <input
                    id="mo2-profile"
                    class="input input-mono"
                    type="text"
                    bind:value={mo2ProfileDir}
                    placeholder="D:\Nordic Souls\profiles\Default"
                  />
                  <button type="button" class="btn btn-ghost btn-sm" onclick={() => pickFolderInto((v) => (mo2ProfileDir = v))}>
                    <FolderOpen size={13} /> Browse
                  </button>
                </div>
              {/if}
              <p class="field-note">The profile's modlist decides which mod folders are scanned.</p>
            </div>
          {/if}
          {#if modManager === "vortex"}
            <div class="field">
              <label for="vortex-staging">Vortex staging folder</label>
              <div class="path-row">
                <input
                  id="vortex-staging"
                  class="input input-mono"
                  type="text"
                  bind:value={vortexStagingFolder}
                  placeholder="C:\Users\…\AppData\Roaming\Vortex\skyrimse\mods"
                />
                <button type="button" class="btn btn-ghost btn-sm" onclick={() => pickFolderInto((v) => (vortexStagingFolder = v))}>
                  <FolderOpen size={13} /> Browse
                </button>
              </div>
            </div>
          {/if}
        </div>
      </section>

      <!-- JSLOT locations — the core setting -->
      <section class="sf-card pad">
        <div class="section-head">
          <h3 class="sf-label">JSLOT locations</h3>
          <div class="head-actions">
            <button type="button" class="btn btn-ghost btn-sm" onclick={applyDefaultRoots}>
              <Sparkles size={13} /> Restore defaults
            </button>
            <button type="button" class="btn btn-ghost btn-sm" onclick={addRoot}>
              <FolderPlus size={13} /> Add folder
            </button>
          </div>
        </div>
        <p class="field-note">
          Every location is scanned recursively — presets in subfolders are always found.
        </p>
        {#if jslotRoots.length === 0}
          <p class="empty-roots">No locations yet. Pick a mod manager above or add a folder.</p>
        {:else}
          <ul class="roots">
            {#each jslotRoots as root (root.id)}
              {@const rootScan = scanInfo?.roots.find((r) => r.root_id === root.id)}
              <li>
                <div class="root-main">
                  <span class="root-label">{root.label}</span>
                  {#if root.kind === "mo2_mods"}
                    <span class="kind-chip">enabled mods only</span>
                  {/if}
                  <span class="root-path mono" title={root.path}>{root.path}</span>
                </div>
                <div class="root-side">
                  {#if rootScan}
                    {#if rootScan.exists}
                      <span class="count mono">{rootScan.count}</span>
                    {:else}
                      <span class="missing">missing</span>
                    {/if}
                  {/if}
                  <button type="button" class="icon-btn" title="Remove this location" aria-label={`Remove ${root.label}`} onclick={() => removeRoot(root.id)}>
                    <Trash2 size={13} strokeWidth={1.6} />
                  </button>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
        <div class="scan-line">
          {#if scanning}
            <span class="detect-note"><RefreshCw size={11} class="spin" /> Scanning…</span>
          {:else if scanInfo}
            <span>
              Found <strong class="mono accent">{scanInfo.total}</strong> JSLOT files across
              <strong class="mono accent">{scanInfo.roots.filter((r) => r.exists).length}</strong> locations.
            </span>
          {:else}
            <span class="field-note">Counts appear after a scan.</span>
          {/if}
          <button type="button" class="btn btn-ghost btn-sm" disabled={scanning} onclick={() => refreshScan(true)}>
            <RefreshCw size={13} /> Rescan
          </button>
        </div>
      </section>

      <!-- Backup destination + snapshots -->
      <section class="sf-card pad">
        <h3 class="sf-label">Backup destination</h3>
        <div class="field">
          <div class="path-row">
            <input
              class="input input-mono"
              type="text"
              bind:value={backupDir}
              placeholder="C:\Users\…\Documents\Lineage\Backups"
              aria-label="Backup destination folder"
            />
            <button type="button" class="btn btn-ghost btn-sm" onclick={() => pickFolderInto((v) => (backupDir = v))}>
              <FolderOpen size={13} /> Browse
            </button>
          </div>
          <p class="field-note">
            Holds full backup archives and the operation snapshots that power Undo and History.
          </p>
        </div>
        {#if backupDirCheck?.problem}
          <div class="note note-danger">{backupDirCheck.problem}</div>
        {/if}
        {#if backupDirCheck?.inside_mod_tree}
          <div class="note note-warning">{backupDirCheck.inside_mod_tree}</div>
        {/if}

        <div class="snapshot-row">
          <div class="field retention">
            <label for="retention">Snapshots to keep</label>
            <input
              id="retention"
              class="input"
              type="number"
              min="1"
              max="200"
              bind:value={snapshotRetention}
            />
          </div>
          <div class="snap-stats">
            {#if snapStats}
              <span class="mono">{snapStats.count}</span> snapshots ·
              <span class="mono">{(snapStats.total_size / 1024).toFixed(0)} KB</span>
            {:else}
              <span class="field-note">No snapshots yet.</span>
            {/if}
          </div>
          <div class="head-actions">
            <button type="button" class="btn btn-ghost btn-sm" onclick={() => (historyOpen = true)}>
              <History size={13} /> History
            </button>
            <button type="button" class="btn btn-ghost btn-sm" disabled={!snapStats || snapStats.count === 0} onclick={onClearSnapshots}>
              <Trash2 size={13} /> Clear snapshots
            </button>
          </div>
        </div>
        <p class="field-note">
          Older snapshots are pruned automatically. Full backup archives are yours — Lineage never deletes them.
        </p>
      </section>

      <!-- Nexus API key -->
      <section class="sf-card pad">
        <h3 class="sf-label">Nexus Mods API key</h3>
        <p class="field-note">
          Used by Find Assets to identify mods and fetch their pages. Your personal key lives at
          <button
            type="button"
            class="inline-link"
            onclick={() => openUrl("https://www.nexusmods.com/settings/api-keys")}
          >nexusmods.com/settings/api-keys</button>
          (Site Preferences → API Keys). Without one, Find Assets still lists every referenced
          asset — just without Nexus matches.
        </p>
        <div class="path-row">
          <input
            class="input input-mono"
            type="password"
            bind:value={apiKeyInput}
            placeholder={maskedKey || "Paste your personal API key"}
            aria-label="Nexus Mods API key"
          />
          <button type="button" class="btn btn-ghost btn-sm" disabled={keyValidating || (apiKeyInput.trim() === "" && !savedApiKey)} onclick={validateKey}>
            <KeyRound size={13} /> {keyValidating ? "Checking…" : "Validate"}
          </button>
          {#if savedApiKey}
            <button type="button" class="btn btn-ghost btn-sm" onclick={clearKey}>Remove</button>
          {/if}
        </div>
        {#if keyValidation}
          <div class="note note-success">
            Key OK — signed in as <strong>{keyValidation.name}</strong>{keyValidation.is_premium ? " (Premium)" : ""}.
            {#if keyValidation.daily_remaining !== null}
              {keyValidation.daily_remaining} daily requests remaining.
            {/if}
          </div>
        {/if}
        {#if keyError}
          <div class="note note-danger">{keyError}</div>
        {/if}
      </section>

      {#if error}
        <div class="note note-danger">{error}</div>
      {/if}
    </div>

    <!-- Footer -->
    <div class="dlg-footer">
      {#if saved}
        <span class="saved-note"><Check size={14} /> Settings saved</span>
      {:else if welcome}
        <button type="button" class="quiet-link" onclick={dismissForever} disabled={saving}>
          Don't show again
        </button>
      {/if}
      <span class="spacer"></span>
      <button type="button" class="btn btn-ghost" onclick={close} disabled={saving || saved}>
        {welcome ? "Skip for now" : "Cancel"}
      </button>
      <button type="button" class="btn btn-primary" onclick={save} disabled={saving || saved}>
        <Check size={14} />
        {saving ? "Saving…" : saved ? "Saved" : welcome ? "Save & Start" : "Save"}
      </button>
    </div>
  </div>
</dialog>

<HistoryDialog bind:open={historyOpen} />

<style>
  .settings-dialog {
    padding: 0;
    border: none;
    background: transparent;
    color: inherit;
    position: fixed;
    inset: 0;
    margin: auto;
    width: fit-content;
    height: fit-content;
    max-width: calc(100vw - 2rem);
    max-height: calc(100vh - 2rem);
    overflow: hidden;
  }
  .settings-dialog::backdrop {
    background: var(--sf-scrim);
    backdrop-filter: blur(6px);
  }
  .settings-dialog[open] .frame {
    animation: zoom-in var(--sf-dur-base) var(--sf-ease) both;
  }
  @keyframes zoom-in {
    from {
      opacity: 0;
      transform: scale(0.97);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  .frame {
    width: 760px;
    max-width: calc(100vw - 2rem);
    max-height: calc(100vh - 3rem);
    display: flex;
    flex-direction: column;
    background: var(--sf-raised);
    border: 1px solid var(--sf-border);
    border-radius: var(--sf-r-xl);
    box-shadow: var(--sf-e3);
    overflow: hidden;
  }
  .dlg-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--sf-line);
  }
  .dlg-title {
    display: flex;
    align-items: baseline;
    gap: 12px;
  }
  .dlg-title h2 {
    margin: 0;
    font-size: 18px;
  }
  .dlg-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 20px;
  }
  .dlg-footer {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 20px;
    border-top: 1px solid var(--sf-line);
    background: var(--sf-surface);
  }
  .spacer {
    flex: 1;
  }
  .saved-note {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--sf-success);
    font-size: 13px;
  }
  .quiet-link {
    background: none;
    border: none;
    color: var(--sf-text-off);
    font-size: 12px;
    cursor: pointer;
    padding: 4px;
  }
  .quiet-link:hover {
    color: var(--sf-text-3);
  }

  .welcome {
    display: flex;
    gap: 16px;
    align-items: center;
    padding: 16px 18px;
    border-color: rgba(139, 125, 255, 0.3);
    background: linear-gradient(135deg, var(--sf-primary-soft), transparent 65%), var(--sf-surface);
  }
  .welcome img {
    width: 44px;
    height: 44px;
    filter: drop-shadow(0 0 12px rgba(139, 125, 255, 0.5));
  }
  .welcome h3 {
    margin: 0 0 4px;
    font-size: 15px;
  }
  .welcome p {
    margin: 0;
    font-size: 12.5px;
    color: var(--sf-text-2);
  }
  .welcome .mono,
  .mono {
    font-family: var(--sf-font-mono);
    font-size: 0.95em;
  }
  .accent {
    color: var(--sf-secondary);
  }

  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 10px;
  }
  .section-head h3 {
    margin: 0;
  }
  .head-actions {
    display: flex;
    gap: 8px;
  }
  .detect-note {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    color: var(--sf-text-3);
  }
  .detect-note strong {
    color: var(--sf-primary-400);
    font-weight: 600;
  }
  :global(.spin) {
    animation: sf-spin 1s linear infinite;
  }

  .manager-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px;
  }
  .manager-card {
    position: relative;
    text-align: left;
    padding: 12px 14px;
    border-radius: var(--sf-r-lg);
    border: 1px solid var(--sf-line);
    background: var(--sf-surface);
    color: var(--sf-text-3);
    cursor: pointer;
    transition:
      border-color var(--sf-dur-fast) var(--sf-ease),
      background var(--sf-dur-fast) var(--sf-ease),
      box-shadow var(--sf-dur-fast) var(--sf-ease);
  }
  .manager-card:hover {
    border-color: var(--sf-border-strong);
    background: var(--sf-raised);
  }
  .manager-card.selected {
    border-color: rgba(139, 125, 255, 0.5);
    background: var(--sf-selected);
    color: var(--sf-primary-400);
    box-shadow: var(--sf-glow);
  }
  .manager-card .card-title {
    margin-top: 8px;
    font-weight: 600;
    font-size: 13px;
    color: var(--sf-text);
  }
  .manager-card.selected .card-title {
    color: var(--sf-primary-200);
  }
  .manager-card p {
    margin: 4px 0 0;
    font-size: 11px;
    line-height: 1.45;
    color: var(--sf-text-3);
  }
  .detected-chip {
    position: absolute;
    top: 8px;
    right: 8px;
    font-size: 9.5px;
    font-weight: 600;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--sf-secondary);
    background: var(--sf-secondary-soft);
    border: 1px solid rgba(69, 215, 239, 0.3);
    border-radius: var(--sf-r-full);
    padding: 1px 7px;
  }

  .pad {
    padding: 16px 18px;
  }
  .fields {
    display: flex;
    flex-direction: column;
    gap: 14px;
    margin-top: 10px;
  }
  .field label {
    display: block;
    margin-bottom: 6px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--sf-text-3);
  }
  .field .hint {
    font-weight: 400;
    text-transform: none;
    letter-spacing: 0;
    color: var(--sf-text-off);
  }
  .path-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .path-row .input {
    min-width: 0;
    flex: 1;
  }
  .field-note {
    margin: 6px 0 0;
    font-size: 11.5px;
    color: var(--sf-text-off);
  }
  .empty-roots {
    font-size: 12.5px;
    color: var(--sf-text-3);
    padding: 10px 0;
  }

  .roots {
    list-style: none;
    margin: 10px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .roots li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    min-height: var(--sf-row);
    padding: 4px 10px;
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-md);
    background: var(--sf-inset);
  }
  .root-main {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .root-label {
    font-size: 12.5px;
    font-weight: 500;
    color: var(--sf-text);
    white-space: nowrap;
  }
  .kind-chip {
    font-size: 9.5px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--sf-primary-400);
    background: var(--sf-primary-soft);
    border-radius: var(--sf-r-full);
    padding: 1px 7px;
    white-space: nowrap;
  }
  .root-path {
    font-size: 11px;
    color: var(--sf-text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .root-side {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .count {
    color: var(--sf-secondary);
    font-size: 11.5px;
  }
  .missing {
    color: var(--sf-warning);
    font-size: 11px;
  }

  .scan-line {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-top: 12px;
    font-size: 12.5px;
    color: var(--sf-text-2);
  }

  .snapshot-row {
    display: flex;
    align-items: end;
    gap: 16px;
    margin-top: 14px;
    flex-wrap: wrap;
  }
  .retention {
    width: 120px;
  }
  .snap-stats {
    font-size: 12px;
    color: var(--sf-text-3);
    padding-bottom: 7px;
  }
  .snap-stats .mono {
    color: var(--sf-secondary);
  }

  .icon-btn {
    width: 26px;
    height: 26px;
    display: grid;
    place-items: center;
    border: 1px solid transparent;
    background: transparent;
    border-radius: var(--sf-r-sm);
    color: var(--sf-text-3);
    cursor: pointer;
    transition:
      background var(--sf-dur-instant) ease,
      color var(--sf-dur-instant) ease;
  }
  .icon-btn:hover {
    background: var(--sf-hover);
    color: var(--sf-text);
  }
  .note {
    margin-top: 10px;
  }
  select.input {
    appearance: auto;
  }
  .inline-link {
    background: none;
    border: none;
    padding: 0;
    font-size: inherit;
    font-family: var(--sf-font-mono);
    color: var(--sf-secondary);
    text-decoration: underline;
    cursor: pointer;
  }
</style>
