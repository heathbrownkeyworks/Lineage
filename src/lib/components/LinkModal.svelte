<script lang="ts">
  import { X, Check } from "lucide-svelte";
  import { librarySaveEntries } from "$lib/tauri";
  import type { LibraryEntry } from "$lib/types";

  type Ref = { kind: string; value: string };
  type Kind = "plugin" | "texture" | "morph";

  type Props = {
    open?: boolean;
    /** References this correction covers (1 for Add Link, N for a card edit). */
    references: Ref[];
    /** Prefill for editing an existing attribution/entry. */
    initialName?: string;
    initialUrl?: string;
    /** All references in the current report, for the live match count. */
    contextRefs?: Ref[];
    /** Called after a successful save. */
    onsaved: () => void;
  };
  let {
    open: isOpen = $bindable(false),
    references,
    initialName,
    initialUrl,
    contextRefs,
    onsaved,
  }: Props = $props();

  let dialog: HTMLDialogElement | undefined = $state();

  let name = $state("");
  let url = $state("");
  let scope = $state<"exact" | "prefix">("exact");
  let pattern = $state("");
  let selectedKind = $state<Kind>("plugin");
  let checked = $state<Set<string>>(new Set());
  let saving = $state(false);
  let error = $state<string | null>(null);

  function refKey(ref: Ref): string {
    return `${ref.kind}|${ref.value}`;
  }

  // Reset the whole working draft every time the dialog opens, re-derived
  // from whatever the caller just set the props to.
  $effect(() => {
    if (!dialog) return;
    if (isOpen) {
      error = null;
      saving = false;
      name = initialName ?? "";
      url = initialUrl ?? "";
      scope = "exact";
      selectedKind = (references[0]?.kind as Kind | undefined) ?? "plugin";
      pattern = references[0]?.value ?? "";
      checked = new Set(references.map(refKey));
      dialog.showModal();
    } else if (dialog.open) {
      dialog.close();
    }
  });

  function toggleRef(ref: Ref) {
    const key = refKey(ref);
    const next = new Set(checked);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    checked = next;
  }

  // Live "matches N references in this report" line. Exact scope just
  // reports how many references are currently selected; prefix scope
  // reports how many context references that prefix would actually catch.
  const matchCount = $derived.by(() => {
    if (!contextRefs || contextRefs.length === 0) return null;
    if (scope === "exact") {
      if (references.length > 0) return checked.size;
      const p = pattern.trim().toLowerCase();
      if (!p) return 0;
      return contextRefs.filter((r) => r.kind === selectedKind && r.value.toLowerCase() === p).length;
    }
    const kind = references.length > 0 ? references[0].kind : selectedKind;
    const p = pattern.trim().toLowerCase();
    if (!p) return 0;
    return contextRefs.filter((r) => r.kind === kind && r.value.toLowerCase().startsWith(p)).length;
  });

  // Mirrors the backend's validate() in src-tauri/src/library.rs so the Save
  // button disables before a round trip — the backend stays authoritative.
  const canSave = $derived.by(() => {
    if (name.trim() === "") return false;
    const u = url.trim();
    if (!(u.startsWith("http://") || u.startsWith("https://"))) return false;
    if (scope === "prefix") return pattern.trim().length >= 3;
    if (references.length > 0) return checked.size > 0;
    return pattern.trim() !== "";
  });

  function buildEntries(): LibraryEntry[] {
    const trimmedName = name.trim();
    const trimmedUrl = url.trim();
    if (scope === "prefix") {
      const kind = (references.length > 0 ? references[0].kind : selectedKind) as Kind;
      return [
        {
          id: "",
          kind,
          pattern: pattern.trim(),
          match_type: "prefix",
          name: trimmedName,
          url: trimmedUrl,
        },
      ];
    }
    if (references.length > 0) {
      const chosen = references.filter((r) => checked.has(refKey(r)));
      return chosen.map((r) => ({
        id: "",
        kind: r.kind as Kind,
        pattern: r.value,
        match_type: "exact" as const,
        name: trimmedName,
        url: trimmedUrl,
      }));
    }
    return [
      {
        id: "",
        kind: selectedKind,
        pattern: pattern.trim(),
        match_type: "exact",
        name: trimmedName,
        url: trimmedUrl,
      },
    ];
  }

  async function save() {
    if (!canSave) return;
    saving = true;
    error = null;
    try {
      await librarySaveEntries(buildEntries());
      onsaved();
      isOpen = false;
    } catch (e) {
      error = friendly(e);
    } finally {
      saving = false;
    }
  }

  function friendly(e: unknown): string {
    return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
  }

  function close() {
    isOpen = false;
  }
</script>

<dialog bind:this={dialog} onclose={() => (isOpen = false)} class="link-dialog">
  <div class="frame">
    <div class="dlg-header">
      <div class="dlg-title">
        <h2 class="sf-display">Link to Library</h2>
        <span class="sf-micro">NAME · URL · MATCH SCOPE</span>
      </div>
      <button type="button" class="icon-btn" onclick={close} aria-label="Close">
        <X size={15} strokeWidth={1.6} />
      </button>
    </div>

    <div class="dlg-body">
      <div class="field">
        <label for="link-name">Name</label>
        <input id="link-name" class="input" type="text" bind:value={name} placeholder="Mod name" />
      </div>
      <div class="field">
        <label for="link-url">Link</label>
        <input
          id="link-url"
          class="input input-mono"
          type="url"
          bind:value={url}
          placeholder="https://www.nexusmods.com/skyrimspecialedition/mods/…"
        />
      </div>

      {#if references.length > 0}
        <div class="field">
          <span class="field-label-text">Applies to</span>
          <div class="scope-choice" role="radiogroup" aria-label="Match scope">
            <label class="scope-chip" class:selected={scope === "exact"}>
              <input type="radio" name="scope" checked={scope === "exact"} onchange={() => (scope = "exact")} />
              These exact references
            </label>
            <label class="scope-chip" class:selected={scope === "prefix"}>
              <input type="radio" name="scope" checked={scope === "prefix"} onchange={() => (scope = "prefix")} />
              Everything starting with…
            </label>
          </div>
        </div>

        {#if scope === "prefix"}
          <div class="field">
            <label for="link-pattern">Prefix pattern</label>
            <input
              id="link-pattern"
              class="input input-mono"
              type="text"
              bind:value={pattern}
              placeholder={references[0]?.value ?? ""}
            />
          </div>
        {:else if references.length > 1}
          <div class="field">
            <span class="field-label-text">References covered</span>
            <ul class="ref-checks">
              {#each references as ref (refKey(ref))}
                <li>
                  <label class="ref-row">
                    <input
                      type="checkbox"
                      checked={checked.has(refKey(ref))}
                      onchange={() => toggleRef(ref)}
                    />
                    <span class="chip-kind">{ref.kind}</span>
                    <span class="mono val">{ref.value}</span>
                  </label>
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      {:else}
        <div class="field-row">
          <div class="field kind-field">
            <label for="link-kind">Kind</label>
            <select
              id="link-kind"
              class="input"
              value={selectedKind}
              onchange={(e) => (selectedKind = e.currentTarget.value as Kind)}
            >
              <option value="plugin">Plugin</option>
              <option value="texture">Texture</option>
              <option value="morph">Morph</option>
            </select>
          </div>
          <div class="field pattern-field">
            <label for="link-pattern-scratch">Pattern</label>
            <input
              id="link-pattern-scratch"
              class="input input-mono"
              type="text"
              bind:value={pattern}
              placeholder="mod_something.esp"
            />
          </div>
        </div>
        <div class="field">
          <span class="field-label-text">Match type</span>
          <div class="scope-choice" role="radiogroup" aria-label="Match type">
            <label class="scope-chip" class:selected={scope === "exact"}>
              <input type="radio" name="scope" checked={scope === "exact"} onchange={() => (scope = "exact")} />
              Exact
            </label>
            <label class="scope-chip" class:selected={scope === "prefix"}>
              <input type="radio" name="scope" checked={scope === "prefix"} onchange={() => (scope = "prefix")} />
              Prefix
            </label>
          </div>
        </div>
      {/if}

      {#if matchCount !== null}
        <p class="match-count mono">
          matches {matchCount} reference{matchCount === 1 ? "" : "s"} in this report
        </p>
      {/if}

      {#if error}
        <div class="note note-danger">{error}</div>
      {/if}
    </div>

    <div class="dlg-footer">
      <span class="spacer"></span>
      <button type="button" class="btn btn-ghost" onclick={close} disabled={saving}>Cancel</button>
      <button type="button" class="btn btn-primary" onclick={save} disabled={saving || !canSave}>
        <Check size={14} />
        {saving ? "Saving…" : "Save"}
      </button>
    </div>
  </div>
</dialog>

<style>
  .link-dialog {
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
  .link-dialog::backdrop {
    background: var(--sf-scrim);
    backdrop-filter: blur(6px);
  }
  .link-dialog[open] .frame {
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
    width: 480px;
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
    padding: 18px 20px;
    display: flex;
    flex-direction: column;
    gap: 14px;
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

  .field label,
  .field-label-text {
    display: block;
    margin-bottom: 6px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--sf-text-3);
  }
  .field-row {
    display: flex;
    gap: 12px;
  }
  .field-row .field {
    flex: 1;
    min-width: 0;
  }
  select.input {
    appearance: auto;
  }

  .scope-choice {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .scope-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border-radius: var(--sf-r-full);
    border: 1px solid var(--sf-line);
    background: var(--sf-surface);
    color: var(--sf-text-2);
    font-size: 12.5px;
    cursor: pointer;
    transition:
      border-color var(--sf-dur-fast) var(--sf-ease),
      background var(--sf-dur-fast) var(--sf-ease),
      color var(--sf-dur-fast) var(--sf-ease);
  }
  .scope-chip:hover {
    border-color: var(--sf-border-strong);
    background: var(--sf-hover);
  }
  .scope-chip.selected {
    border-color: rgba(139, 125, 255, 0.5);
    background: var(--sf-selected);
    color: var(--sf-primary-400);
  }
  .scope-chip input[type="radio"] {
    width: 13px;
    height: 13px;
    margin: 0;
    accent-color: var(--sf-primary);
  }

  .ref-checks {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 180px;
    overflow-y: auto;
  }
  .ref-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 10px;
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-md);
    background: var(--sf-inset);
    cursor: pointer;
    font-size: 12px;
  }
  .ref-row input[type="checkbox"] {
    width: 14px;
    height: 14px;
    flex: 0 0 auto;
    accent-color: var(--sf-primary);
  }
  .chip-kind {
    font-size: 9px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--sf-primary-400);
  }
  .val {
    color: var(--sf-text);
    overflow-wrap: anywhere;
  }
  .mono {
    font-family: var(--sf-font-mono);
  }

  .match-count {
    margin: 0;
    font-size: 11.5px;
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
  }
  .icon-btn:hover {
    background: var(--sf-hover);
    color: var(--sf-text);
  }
</style>
