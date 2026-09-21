<script lang="ts">
  import { Copy, GitCompare, Trash2, CheckCircle2 } from "lucide-svelte";
  import PageHeader from "$lib/components/PageHeader.svelte";
  import PresetPicker from "$lib/components/PresetPicker.svelte";
  import ProgressBar from "$lib/components/ProgressBar.svelte";
  import { comparePresets, findDuplicatePresets, removeDuplicates } from "$lib/tauri";
  import { bumpPresets } from "$lib/stores/app.svelte";
  import type {
    CompareProgress,
    Comparison,
    DuplicateGroup,
    DuplicatesReport,
    PresetRef,
    RemoveOutcome,
  } from "$lib/types";

  const errorText = (e: unknown) => (typeof e === "string" ? e : String(e));
  const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;
  const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
  /** The mod a preset belongs to, or its folder. */
  function where(path: string): string {
    const parts = path.split(/[\\/]/);
    const mods = parts.findIndex((p) => p.toLowerCase() === "mods");
    if (mods >= 0 && mods + 1 < parts.length - 1) return parts[mods + 1];
    return parts[parts.length - 2] ?? "";
  }

  // ---- Duplicates ---------------------------------------------------------

  let finding = $state(false);
  let findProgress = $state<CompareProgress | null>(null);
  let dupes = $state<DuplicatesReport | null>(null);
  let dupesError = $state<string | null>(null);
  /** Paths ticked for removal. */
  let marked = $state<Set<string>>(new Set());
  let confirming = $state(false);
  let removing = $state(false);
  let removed = $state<RemoveOutcome | null>(null);

  async function findDupes() {
    finding = true;
    dupesError = null;
    findProgress = null;
    dupes = null;
    marked = new Set();
    removed = null;
    confirming = false;
    try {
      dupes = await findDuplicatePresets((p) => (findProgress = p));
    } catch (e) {
      dupesError = errorText(e);
    } finally {
      finding = false;
      findProgress = null;
    }
  }

  /** At least one preset stays in every group — and a file can sit in both
   *  an exact group and a same-face group, so every group it's in counts. */
  function canMark(path: string): boolean {
    if (marked.has(path) || !dupes) return true;
    return [...dupes.exact, ...dupes.same_face]
      .filter((g) => g.presets.some((p) => p.path === path))
      .every((g) => g.presets.filter((p) => !marked.has(p.path)).length > 1);
  }

  function toggleMark(path: string) {
    const next = new Set(marked);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    marked = next;
    confirming = false;
  }

  async function removeMarked() {
    if (!confirming) {
      confirming = true;
      return;
    }
    removing = true;
    dupesError = null;
    try {
      const groups = dupes
        ? [
            ...dupes.exact.map((g) => ({ kind: "exact" as const, members: g.presets.map((p) => p.path) })),
            ...dupes.same_face.map((g) => ({ kind: "same_face" as const, members: g.presets.map((p) => p.path) })),
          ]
        : [];
      const outcome = await removeDuplicates(groups, [...marked]);
      removed = outcome;
      const gone = new Set(outcome.removed);
      const prune = (groups: DuplicateGroup[]) =>
        groups
          .map((g) => ({ presets: g.presets.filter((p) => !gone.has(p.path)) }))
          .filter((g) => g.presets.length > 1);
      if (dupes) {
        dupes = {
          ...dupes,
          total: dupes.total - gone.size,
          exact: prune(dupes.exact),
          same_face: prune(dupes.same_face),
          near_twins: dupes.near_twins.filter((t) => !gone.has(t.left.path) && !gone.has(t.right.path)),
        };
      }
      marked = new Set([...marked].filter((p) => !gone.has(p)));
      if ((left && gone.has(left)) || (right && gone.has(right))) {
        if (left && gone.has(left)) left = null;
        if (right && gone.has(right)) right = null;
        comparison = null;
      }
      bumpPresets();
    } catch (e) {
      dupesError = errorText(e);
    } finally {
      removing = false;
      confirming = false;
    }
  }

  // ---- Compare two --------------------------------------------------------

  let left = $state<string | null>(null);
  let right = $state<string | null>(null);
  let comparing = $state(false);
  let comparison = $state<Comparison | null>(null);
  let compareError = $state<string | null>(null);
  let showSame = $state(false);
  let compareCard = $state<HTMLElement | null>(null);
  let compareRequest = 0;

  async function runCompare() {
    if (!left || !right) return;
    const id = ++compareRequest;
    comparing = true;
    compareError = null;
    try {
      const result = await comparePresets(left, right);
      if (id === compareRequest) comparison = result;
    } catch (e) {
      if (id === compareRequest) compareError = errorText(e);
    } finally {
      if (id === compareRequest) comparing = false;
    }
  }

  function pick(side: "left" | "right", path: string) {
    if (side === "left") left = path;
    else right = path;
    comparison = null;
    void runCompare();
  }

  function compareThese(a: PresetRef, b: PresetRef) {
    left = a.path;
    right = b.path;
    comparison = null;
    void runCompare();
    compareCard?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  function headline(c: Comparison): string {
    if (c.same_bytes) return "Identical files, byte for byte.";
    if (c.same_face && c.body_differences === 0) return "The same preset, saved differently — nothing that shows differs.";
    if (c.same_face) return `The same face. Only body & placement differ (${plural(c.body_differences, "item")}).`;
    return `${plural(c.face_differences, "face difference")}${
      c.body_differences ? `, ${plural(c.body_differences, "body difference")}` : ""
    }.`;
  }
</script>

{#snippet group(g: DuplicateGroup)}
  <li class="group">
    <ul class="members">
      {#each g.presets as p (p.path)}
        <li>
          <label class="member" class:gone={marked.has(p.path)} title={p.path}>
            <input
              type="checkbox"
              checked={marked.has(p.path)}
              disabled={removing || !canMark(p.path)}
              onchange={() => toggleMark(p.path)}
            />
            <span class="name">{p.file_name}</span>
            <span class="where">{where(p.path)}</span>
          </label>
        </li>
      {/each}
    </ul>
    <button type="button" class="btn btn-ghost btn-sm" onclick={() => compareThese(g.presets[0], g.presets[1])}>
      <GitCompare size={12} /> Compare
    </button>
  </li>
{/snippet}

<PageHeader
  title="Compare"
  subtitle="Find duplicate and near-identical presets, and see exactly what differs between any two: head parts, sliders, sculpt, tints, overlays and body."
/>

<div class="content">
  <section class="sf-card pad">
    <div class="row-head">
      <h2 class="sf-label">Duplicates</h2>
      <button type="button" class="btn btn-primary btn-sm" disabled={finding || removing} onclick={findDupes}>
        <Copy size={13} />
        {dupes ? "Look again" : "Find duplicates"}
      </button>
    </div>
    <p class="quiet">
      Every preset in your JSLOT locations, grouped three ways: exact copies, the same face with
      different body data, and near-twins a few details apart.
    </p>
    {#if finding}
      <ProgressBar current={findProgress?.current ?? 0} total={findProgress?.total ?? 0} label={findProgress?.detail ?? ""} />
    {/if}
    {#if dupesError}
      <div class="note note-danger">{dupesError}</div>
    {/if}
    {#if removed}
      <div class="note note-success done">
        <CheckCircle2 size={14} />
        <span>
          Removed {plural(removed.removed.length, "preset")}. They're in snapshot
          <span class="mono">{removed.snapshot_id}</span> — restore them from History (Settings →
          History).{removed.failed.length ? ` ${removed.failed.length} couldn't be removed.` : ""}
        </span>
      </div>
    {/if}

    {#if dupes && !finding}
      <p class="summary">
        {plural(dupes.total, "preset")} · {plural(dupes.exact.length, "exact copy group")} ·
        {plural(dupes.same_face.length, "same-face group")} · {plural(dupes.near_twins.length, "near-twin")}
      </p>

      {#if dupes.exact.length > 0 || dupes.same_face.length > 0}
        <div class="remove-bar">
          <span class="quiet">
            Tick the copies to remove. One in every group always stays; a snapshot is taken first.
          </span>
          <button
            type="button"
            class="btn btn-sm"
            class:btn-danger={confirming}
            class:btn-ghost={!confirming}
            disabled={marked.size === 0 || removing}
            onclick={removeMarked}
          >
            <Trash2 size={12} />
            {confirming ? `Confirm: remove ${plural(marked.size, "preset")}` : `Remove ${marked.size} ticked…`}
          </button>
        </div>
      {/if}

      <details open={dupes.exact.length > 0}>
        <summary class="sf-label">Exact copies · {dupes.exact.length}</summary>
        {#if dupes.exact.length === 0}
          <p class="quiet">No byte-identical presets.</p>
        {:else}
          <ul class="groups">
            {#each dupes.exact as g (g.presets.map((p) => p.path).join("|"))}{@render group(g)}{/each}
          </ul>
        {/if}
      </details>

      <details>
        <summary class="sf-label">Same face, different body · {dupes.same_face.length}</summary>
        <p class="quiet">
          Every head part, slider, sculpt, tint and overlay matches; they differ only in body morphs,
          skeleton, weight or height. Often an intended variant — Compare shows what differs.
        </p>
        <ul class="groups">
          {#each dupes.same_face as g (g.presets.map((p) => p.path).join("|"))}{@render group(g)}{/each}
        </ul>
      </details>

      <details>
        <summary class="sf-label">Near-twins · {dupes.near_twins.length}</summary>
        <p class="quiet">
          Faces 1 to 5 details apart that share a head mesh, closest first — usually versions of one
          character. Never removed from here.
        </p>
        <ul class="twins">
          {#each dupes.near_twins as t (t.left.path + "|" + t.right.path)}
            <li class="twin">
              <span class="pair" title={`${t.left.path}\n${t.right.path}`}>
                <span class="name">{t.left.file_name}</span>
                <span class="dim">~</span>
                <span class="name">{t.right.file_name}</span>
              </span>
              <span class="what" title={t.what.join("\n")}>{t.what.join(", ")}</span>
              <button type="button" class="btn btn-ghost btn-sm" onclick={() => compareThese(t.left, t.right)}>
                <GitCompare size={12} /> Compare
              </button>
            </li>
          {/each}
        </ul>
      </details>

      {#if dupes.unreadable.length > 0}
        <p class="quiet">
          {plural(dupes.unreadable.length, "preset")} couldn't be read and {dupes.unreadable.length === 1
            ? "was"
            : "were"} skipped — Readiness lists them.
        </p>
      {/if}
    {/if}
  </section>

  <section class="sf-card pad" bind:this={compareCard}>
    <h2 class="sf-label">Compare two presets</h2>
    <div class="pickers">
      <div class="side">
        <p class="side-label">Left{left ? ` · ${fileName(left)}` : ""}</p>
        <div class="picker-box"><PresetPicker selected={left} onpick={(p) => pick("left", p)} /></div>
      </div>
      <div class="side">
        <p class="side-label">Right{right ? ` · ${fileName(right)}` : ""}</p>
        <div class="picker-box"><PresetPicker selected={right} onpick={(p) => pick("right", p)} /></div>
      </div>
    </div>

    {#if compareError}
      <div class="note note-danger">{compareError}</div>
    {:else if comparing}
      <p class="quiet">Comparing…</p>
    {:else if comparison}
      <div class="result-head">
        <p class="headline" class:same={comparison.same_face}>{headline(comparison)}</p>
        <label class="check">
          <input type="checkbox" bind:checked={showSame} />
          <span>Show what's the same</span>
        </label>
      </div>
      {#each comparison.sections as s (s.section)}
        {#if showSame || s.differences > 0}
          <div class="diff-section">
            <p class="sf-label">
              {s.label} · {s.differences > 0 ? `${s.differences} of ${s.items.length} differ` : `${s.items.length} the same`}
            </p>
            <table class="diff">
              <tbody>
                {#each s.items.filter((i) => showSame || !i.same) as item (item.key)}
                  <tr class:changed={!item.same}>
                    <td class="k" title={item.key}>{item.key}</td>
                    <td class="v mono" title={item.left ?? ""}>{item.left ?? "—"}</td>
                    <td class="v mono" title={item.right ?? ""}>{item.right ?? "—"}</td>
                    <td class="note-cell">{item.note ?? ""}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {/each}
    {:else}
      <p class="quiet">Pick a preset on each side, or use Compare on any group above.</p>
    {/if}
  </section>
</div>

<style>
  .content {
    padding: 20px 28px 28px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .pad {
    padding: 16px 20px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .row-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  h2,
  .row-head h2 {
    margin: 0;
  }
  .quiet {
    margin: 0;
    color: var(--sf-text-3);
    font-size: 12.5px;
    line-height: 1.5;
  }
  .summary {
    margin: 0;
    font-size: 12.5px;
    color: var(--sf-text-2);
  }
  .mono {
    font-family: var(--sf-font-mono);
  }
  .dim {
    color: var(--sf-text-3);
  }
  .done {
    display: flex;
    gap: 8px;
    align-items: flex-start;
  }
  .remove-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 10px;
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-md);
  }
  details summary {
    cursor: pointer;
    margin: 4px 0 6px;
  }
  details > .quiet {
    margin-bottom: 6px;
  }
  .groups,
  .members,
  .twins {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .groups {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 420px;
    overflow: auto;
  }
  .group {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 8px;
    border: 1px solid var(--sf-line);
    border-radius: var(--sf-r-md);
  }
  .members {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  .member {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
    cursor: pointer;
  }
  .member.gone .name {
    text-decoration: line-through;
    color: var(--sf-text-3);
  }
  .name {
    color: var(--sf-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .where {
    font-size: 11px;
    color: var(--sf-text-3);
    white-space: nowrap;
  }
  .twins {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 420px;
    overflow: auto;
  }
  .twin {
    display: grid;
    grid-template-columns: minmax(0, 1.3fr) minmax(0, 1fr) auto;
    align-items: center;
    gap: 12px;
    padding: 3px 8px;
    border-radius: var(--sf-r-sm);
    font-size: 12.5px;
  }
  .twin:hover {
    background: var(--sf-hover);
  }
  .pair {
    display: flex;
    gap: 6px;
    min-width: 0;
  }
  .what {
    font-size: 11.5px;
    color: var(--sf-text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pickers {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }
  .side-label {
    margin: 0 0 6px;
    font-size: 12px;
    color: var(--sf-text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .picker-box {
    height: 280px;
    display: flex;
    flex-direction: column;
  }
  .picker-box :global(.picker) {
    flex: 1;
  }
  .result-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .headline {
    margin: 0;
    font-size: 13px;
    color: var(--sf-warning);
  }
  .headline.same {
    color: var(--sf-success);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--sf-text-2);
    cursor: pointer;
  }
  .diff-section .sf-label {
    margin: 6px 0 4px;
  }
  .diff {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
    font-size: 12px;
  }
  .diff td {
    padding: 3px 8px;
    border-bottom: 1px solid var(--sf-line);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .diff .k {
    width: 28%;
    color: var(--sf-text-2);
  }
  .diff .v {
    width: 28%;
    font-size: 11.5px;
    color: var(--sf-text-3);
  }
  .diff tr.changed .v {
    color: var(--sf-text);
  }
  .diff tr.changed .k {
    color: var(--sf-warning);
  }
  .note-cell {
    width: 16%;
    font-size: 11px;
    color: var(--sf-text-3);
  }
</style>
