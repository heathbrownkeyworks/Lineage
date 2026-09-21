# Lineage — Build Notes

What was reused, what was confirmed against real data, and where the spec's
references were thin.

---

## Reused from Visage (`E:\Modding\Projects\visage`)

- **Window border treatment** — Visage kills the light DWM hairline by calling
  `DwmSetWindowAttribute(DWMWA_BORDER_COLOR = 34)` with a COLORREF matching its
  chrome color. Lineage ports `tint_window_border()` verbatim
  ([lib.rs](src-tauri/src/lib.rs)), tinted to Starfall `--sf-void` `#07070E`
  (COLORREF `0x000E0707`). `decorations: false` in tauri.conf.json, same as
  Visage.
- **Titlebar structure** — same layout and control order as Visage's
  `TitleBar.svelte`: brand mark + wordmark + tagline on the left; upper right
  is settings gear → divider → minimize / maximize / close, 26 px buttons,
  danger-tinted close hover. The defensive `win()` wrapper (chrome still
  renders outside the Tauri runtime) is Visage's pattern too.
- **Sidebar pattern** — nav column on `--sf-void` (Visage: `ef-bg-sunken`),
  32 px items, active route = accent text + selected wash + a 2 px accent rule
  that scales in from the left (Visage's "ember rule", re-colored violet), and
  the bottom "live card" with mono microcopy.
- **Settings conventions** — `AppSettings` persisted as pretty JSON with
  serde defaults so old files load cleanly; `get_settings`/`save_settings`
  commands; `Result<T, String>` command errors carrying human-readable
  messages; heavy work on `spawn_blocking` with `tauri::ipc::Channel`
  progress events. All Visage's conventions.
- **Mod-manager detection logic** — Visage's `detect_mod_manager` precedence
  is kept: a Vortex deployment manifest in `Data` wins, then MO2 footprints,
  else Manual. Visage's `discover_mo2_profiles` (a profile folder must carry
  modlist.txt) and its `@ByteArray(...)` unwrapping needs were ported into
  [detect.rs](src-tauri/src/detect.rs). Lineage extends detection for its
  zero-config first run (Visage asks for folders first): Skyrim install via
  Bethesda/Steam/GOG registry keys + `libraryfolders.vdf`, MO2 via the
  registry `CurrentInstance`, the `nxm://` protocol handler (portable
  installs — this is what finds `D:\Nordic Souls`), `%LOCALAPPDATA%\ModOrganizer`
  instances, and common paths; Vortex via `%APPDATA%\Vortex`.
- **Layout/motion lessons** — route transition is opacity-only and modal
  entrance is opacity+scale-only; Visage's comments record that translates
  flash a transient scrollbar. Kept both.
- **First-run flow** — welcome-mode Settings dialog that opens until
  configured or dismissed (`setup_dismissed`), with "Don't show again".

## Starfall gaps / notes

- **Geomini** exists on Fontsource (`@fontsource/geomini@5.3.0`) — bundled
  locally along with IBM Plex Sans and JetBrains Mono rather than using the
  Google Fonts `<link>` from starfall-tailwind.md, so the app renders
  correctly offline. No token deviation.
- Starfall defines no **checkbox / list-row selection** component. Batch
  Remove derives them from existing tokens: `--sf-row` height rows,
  `accent-color: --sf-primary` checkboxes, `--sf-hover` row hover.
- Starfall defines no **status note** component (success/warning/danger
  inline messages). Derived from the `*-soft` color tokens + `--sf-r-md`
  (`.note-*` in app.css).
- No **native `<select>`** styling exists in Starfall; the MO2 profile picker
  uses the `.input` recipe on a bare select rather than inventing a custom
  dropdown.
- The Tailwind v4 `@theme` mapping is exactly the block from
  starfall-tailwind.md; tokens.css is vendored unmodified at
  `src/lib/starfall/tokens.css`.

## Confirmed JSLOT format (from 1,840 real presets on this machine)

Top-level keys observed (all presets share this shape; `transforms` appears
only in some):

`actor` (`hairColor`, `headTexture` "Plugin|FormID", `weight`), `bodyMorphs`,
`faceTextures` (`{index, texture}`), `headParts` (`{formId, formIdentifier
"Plugin.esp|XXXXXX", type}`), `modNames` (plugin name array), `mods`
(`{index, name}`), `morphs` (`{custom: [{name, value}], …}`), `overrides`
(`{node, values: [{data, index, key, type}]}`), `tintInfo` (`{color, index,
texture}`), `transforms`, `version` (`{signature, skseVersion, …}`).

- **`bodyMorphs`** is one of the things Clean Preset strips:
  `[{keys: [{key: "XPMSE.esp", value: n}], name: "MorphName"}]`. It includes
  BodySlide sliders *and* XPMSE skeleton morphs — both are body morph data
  RaceMenu re-applies on preset load. The candidate-key list is the
  `BODY_MORPH_KEYS` constant in [jslot.rs](src-tauri/src/jslot.rs); only
  `bodyMorphs` is confirmed in the wild so far.
- **Clean Preset works per node, not per section** ([clean.rs](src-tauri/src/clean.rs)).
  `overrides` holds body tattoos *and* face makeup, and `transforms` holds
  height *and* head scale, so the entry's `node` name decides what goes.
  Across the 1,840-preset collection: real body/hands/feet overlays in 223 /
  67 / 61 presets, real face overlays in 108; 244 distinct transform nodes.
  Categories (defaults agreed with Heath, 2026-09-20): body morphs, body
  overlays (`Body`/`Hands`/`Feet [OvlN]`), height & skeleton (every
  transform node not otherwise claimed — root, spine, limbs, fingers, butt,
  breasts, genitals, tail), weapon & camera (any node naming a weapon,
  shield, quiver, bolt or camera, `HDT`/`CME` variants included) — all on by
  default; head & neck (`NPC Head`, `Head MagicNode`, neck) offered but off.
  **Face overlays are not a category** — no checkbox can strip a face, so no
  batch run can. A section emptied by a clean is dropped (RaceMenu writes a
  preset without one that way); one the author left empty is not touched.

### Formatting dialects (matters for byte preservation)

Real presets come in two layouts, plus one oddball variant:

1. **RaceMenu native** (jsoncpp StyledWriter): 3-space indent,
   `"key" : value`, keys alphabetically sorted, LF endings, trailing newline,
   short scalar arrays inlined as `[ 1, 2, 3 ]` when the line fits 74 cols.
2. **Converted presets** (Python-style `json.dumps(indent=2)`): 2-space
   indent, `"key": value`, arrays always multiline, no trailing newline.
3. **Mimic-style variant of (1)**: inlines number arrays but always breaks
   string arrays (2 files in the corpus, e.g. exported by the Mimic mod).

Lineage detects the dialect per file and writes it back in kind.
**serde_json cannot do this losslessly** — even with `arbitrary_precision`
it normalizes `-0` → `0` and `1e5` → `1e+5`, and re-encodes string escapes.
So writes go through a small raw-token tree
([rawjson.rs](src-tauri/src/rawjson.rs)) that keeps every scalar as its
verbatim source slice; serde_json remains the validator and the analysis
parser.

**Verified:** `cargo test roundtrip_against_real_presets` parses and
re-serializes the user's real corpus and requires byte-identical output —
currently 1,349 of 1,350 files round-trip exactly. The one exception is a
hand-edited file with an irregular 9-space indent on a single line; no layout
algorithm reproduces that, `inspect` reports it as
`roundtrip_faithful: false`, and the UI says its whitespace will be
normalized. This is the only unavoidable reformatting case found.

## Confirmed Nexus API shapes (from Nexus's own node client, 2026-08)

- Base `https://api.nexusmods.com/v1`, API key in the `apikey` header.
- `GET /v1/users/validate` → `{user_id, name, email, is_premium, …}`.
- `GET /v1/games/skyrimspecialedition/mods/md5_search/{md5}` → array of
  `{mod: {...}, file_details: {...}}`; **404 = hash unknown** (cached as a
  miss so it isn't re-asked for 30 days).
- `GET /v1/games/skyrimspecialedition/mods/{id}` → mod info incl.
  `picture_url`, `author`, `version`, `summary` (cached 24 h).
- Rate limits in `x-rl-daily-remaining` / `x-rl-hourly-remaining` (+ `-reset`
  variants); exhaustion is HTTP 429. Lineage surfaces the reset time and
  keeps returning locally-resolved results when limited.
- **Still no search-by-name endpoint in the public v1 REST API** (a GraphQL
  v2 exists but is out of scope) — unresolved references stay in the Unknown
  list, as the spec requires.

## Confirmed MO2 metadata (from the live `D:\Nordic Souls` instance)

- `ModOrganizer.ini` `[General]`: `gamePath=@ByteArray(D:\\…)` (backslashes
  doubled), `selected_profile=@ByteArray(Name)`. `[Settings]` *may* carry
  `base_directory` / `mod_directory` / `profile_directory` with a `%BASE_DIR%`
  placeholder; this instance uses the defaults (`<instance>\mods`,
  `<instance>\profiles`, `<instance>\overwrite`).
- Profile `modlist.txt`: `+Name` = enabled, `-Name` = disabled, one top-level
  mod folder per line.
- Mod `meta.ini` `[General]`: `modid=` (Nexus mod id; `-1`/`0` for non-Nexus
  installs), `version=`, `installationFile=`, `repository=Nexus`; and
  `[installedFiles]` `1\modid`, `1\fileid`. Lineage reads `modid` and treats
  `<= 0` as "not a Nexus install" (falls through to the MD5 path).

## Vortex notes (no live Vortex install on this machine)

- Deployment manifest name **`vortex.deployment.json`** in the game `Data`
  folder — confirmed from Visage's shipped detection code, which is in
  production use. Schema handled defensively: Lineage reads the `files`
  array's `relPath` (deployed path relative to Data) and `source` (staging
  mod folder name) and ignores everything else; a missing/unreadable manifest
  degrades to the other resolution steps.
- Staging folder names usually end with the Nexus download suffix
  (`Name-<modid>-<version>…`); Lineage parses the mod id out heuristically
  and verifies via the mod-info endpoint when a key is present.
- **Worth re-verifying against a real Vortex install** before shipping to
  Vortex users.

## Deviations & implementation notes

- **"Every enabled mod folder in the active profile" as one root** — instead
  of materializing hundreds of per-mod roots, an MO2 mods folder is a single
  root of kind `mo2_mods`; the scanner walks only top-level folders that are
  enabled in the active profile's modlist.txt. Semantically identical, keeps
  the locations list readable, and the "across N locations" count honest.
- **reqwest TLS** — the spec suggested `reqwest (rustls)`. reqwest 0.13's
  `rustls` feature now defaults to the aws-lc-rs provider, which needs NASM
  to build on Windows MSVC (not installed here). Lineage uses
  `rustls-no-provider` + the `ring` provider installed at startup — still
  rustls, no extra toolchain.
- **Atomic writes** — Windows `std::fs::rename` can't replace an existing
  file, so the swap uses `MoveFileExW(MOVEFILE_REPLACE_EXISTING |
  MOVEFILE_WRITE_THROUGH)`; the temp file lives in the target folder (an
  atomic same-volume replace requires it) and is removed on every failure
  path. Non-Windows builds fall back to `rename`.
- **Snapshot zip entries** are `NNNN-<filename>` (index-prefixed) rather than
  full relative paths — same-named presets from different mods can share one
  snapshot; the JSON sidecar carries the absolute restore path for each entry.
- **Backups carry a manifest; restore works without one.** Entries are
  `<sanitized root label>/<rel path>`, which is enough to restore only while
  the label still maps to a configured root. New backups add
  `lineage-manifest.json` (written last, so a failed backup never carries
  one) recording each preset's root id, label, relative path and original
  absolute path. `restore.rs` resolves by root id first — it survives the
  user renaming a label — then label; archives from before manifests fall
  back to matching the top folder against `backup::sanitize_label` of each
  root. Two roots that sanitize to the same label are left unmapped rather
  than guessed, because which one the writer suffixed `-2` can't be
  recovered from today's settings.
- **Restore safety is enforced server-side.** The UI sends zip entry names,
  never paths; targets are re-resolved and re-classified at restore time. A
  target must sit inside a *currently configured* root: the manifest's
  recorded absolute path is informational and never written to, and any
  relative path with a non-plain component (`..`, `.`, root, drive prefix) is
  refused — so no archive, hand-made or not, can aim a write elsewhere.
  Files a restore would overwrite are snapshotted first (operation
  `restore-backup`, undoable from History); if that snapshot fails the
  restore aborts before touching anything. Missing presets are recreated
  only when their folder still exists — a vanished folder under an MO2 root
  is a removed mod, and recreating it would conjure a mod MO2 never
  installed (same rule as snapshot restore).
- **The backup nudge counts coverage, not age.** An old backup of presets
  nobody has touched is still complete, so age alone would just nag.
  `backup::changed_since_backup` counts presets on disk the last backup
  doesn't cover: ones absent from the archive, plus ones modified after it.
  Membership is by the archive's contents, never timestamps — MO2 keeps a
  file's original mtime when it installs from an archive, so a preset pack
  installed today can look years older than the backup. Deleted presets
  don't count (the backup still has them), and neither do presets in mods
  disabled since the backup (the scan only walks enabled mods). Every page
  that writes presets or makes a backup calls `bumpPresets()` so the banner
  and the sidebar dot never go stale mid-session; "Not now" lasts for the
  session only, because a nudge that could be silenced for good would stop
  being a safety net.
- **Vanilla skin textures are never requirements.** `faceTextures` records the
  head TextureSet slots and `tintInfo` the vanilla tint masks, so nearly every
  preset references `Actors\Character\<Race>\FemaleHead*.dds` and
  `Actors\Character\Character Assets\TintMasks\*`. Every skin/warpaint mod
  replaces those files *in place* at the same paths, so the path identifies
  nothing — and resolving it used to name whichever replacer the scanning
  machine had installed, making the "requirements" for one preset differ per
  user. `assets.rs::is_vanilla_skin_texture` drops both classes during
  extraction (into the vanilla list, not silently), before any lookup runs.
  The skin rule demands a *known vanilla race folder with the file directly
  inside it*, because mods do ship within that tree: Racial Skin Variance at
  `Actors\Character\RSV\<Race>\` (seed entry, nexus/81668), overlay packs at
  `Character Assets\Overlays\`, complexions at `Female\FaceDetails\`.
- **Texture attribution** — a texture path that resolves to no loose file
  anywhere is first offered to the asset library; a named entry wins, because
  the library is explicit knowledge and the vanilla gate below it is only a
  guess (and a guess that overrides a correct entry can't be fixed from the
  UI). Otherwise a path starting with `actors\character\` is classified
  "base game" (vanilla BSAs aren't parsed); anything else goes to Unknown.
- **Morph names** never resolve to files; they're always listed under Unknown
  with a best-guess origin hint for recognizable prefixes (`EFM_`,
  `ECE_`/`CME_`, `XPMSE*`).

## Asset Library

- **Layered storage.** A seed list compiled into the binary via
  `include_str!` ([library.rs](src-tauri/src/library.rs),
  `src-tauri/resources/seed-library.json`) plus a user-editable
  `library.json` in the app config dir
  (`%APPDATA%\com.coldsun.lineage\library.json`). `library::load_from_dir`
  parses and merges both at report time; the seed file itself is never
  written to at runtime, only read.
- **The Unknown queue is grouped, not flat.** Collection Review over the
  whole collection leaves roughly a thousand unattributed references, and one
  row per reference is a thousand modal round-trips — enough that the queue
  simply doesn't get finished. `assets.rs::group_unknown` collapses them into
  the library entries that would cover them: ~700 texture paths become ~70
  groups, 200-odd morphs become ~20. Three rules, in order:
  1. **Mod folder** — the first path segment that isn't a shared container
     (`GENERIC_PATH_SEGMENTS`: actors, character, character assets, overlays,
     data, textures). `Actors\Character\Overlays\Koralina_Male\` is one mod
     however deep its own subfolders go.
  2. **Filename prefix** — when every folder is generic the file is loose in
     a shared directory, and its own leading token is the only handle:
     `Actors\empyreancs_f_10_a.dds` → `Actors\empyreancs_`. Morph families
     (`EXPR_`, `SPG_`) group the same way, which is how the seed entries
     already identify slider packs.
  3. **Neither** — one exact-match group of its own. Nothing is generalized
     on a guess.
  Plugin names have no shared structure, so each is its own group.
- **`Data\Textures\` is a spelling, not a mod.** Presets reference the same
  mod's textures both with and without that lead-in.
  `library::normalize_ref` strips it (and normalizes case and slashes) on
  both sides of every comparison, so one folder entry claims both spellings.
  Before that, a grouped entry would have failed to match some of the very
  references it was created from. `assets.rs` shares the same function
  deliberately — if the grouping and the matching disagreed, a group would
  display a pattern that doesn't cover it.
- **Requirements export works on the presets as they'll ship.**
  [requirements.rs](src-tauri/src/requirements.rs) resolves every reference
  across a chosen set of presets once — the case between Find Assets (one
  preset) and Collection Review (everything) — and counts, per mod, the
  distinct presets needing it (a union, never a sum). With clean categories
  it cleans each preset in memory first, never on disk: a cleaned-away body
  tattoo's texture mod is no longer a requirement. RaceMenu leads every list
  (every JSLOT needs it, none references it), named through the library.
  Requirements sharing a link merge into one line, named by the shared part
  of their names (ECE_ and CME_ both → "Enhanced Character Edit SE").
  Unidentified references are left out of the rendered text and shown in
  the UI, so a pack never ships a silently short list.
- **Rendering lives in Rust so the house style is tested.** Forge BBCode per
  ColdSun's Forge house style, as on Horde's shipped page: amber `size=5`
  header, `[*][url=…]Name[/url] - used by N of M presets`, no usage note when
  every preset needs a mod. Em and en dashes in Nexus titles become hyphens;
  square brackets in names become parentheses, because Nexus strips unknown
  tags and `[Dint999] HairPack02` would lose its author.
- **`meta.ini` can mislabel the game.** An MO2 SSE instance records
  `gameName=SkyrimSE` even for a mod downloaded from Oldrim Nexus (Kai's
  makeup: Oldrim mod 72955, recorded as SSE 72955 with an empty `url=`), so
  a URL built from it points at an unrelated SSE page. Only a *user* library
  entry beats `meta.ini`; seed entries only fill gaps.
- **The release packager shares the Requirements page's selection.**
  [package.rs](src-tauri/src/package.rs) zips the same presets with the
  same clean categories the Requirements list was built from, so the Nexus
  page can't describe a different pack from the zip. A preset's zip path is
  `SKSE/Plugins/CharGen/Presets/` + whatever follows the last
  `CharGen\Presets` in its source path (Miggyluv's `Female\`/`Male\`,
  Anuketh's `[Anuketh Presets]\` survive); anything else goes in by name.
  Head exports are `<name>.nif`/`.dds` in the CharGen folder holding that
  Presets folder: 24 of 40 installed preset mods ship them, 1,105 of 1,350
  ColdSun presets have one. They average ~31 MB (uncompressed 2K/4K tint
  masks), so they're streamed into the zip, and a whole-collection pack
  with them would run to tens of GB. Two different files on one zip path
  refuse the pack rather than overwrite: ColdSun's own collection has
  `CharGen\Exported\Colson.jslot` beside `Presets\Colson.jslot`. The zip is
  written beside its destination as a `.lineage-partial`, reopened and
  checked (entry list, every preset parses, no chosen category left), then
  renamed. Real run: 1,327 of 1,350 ColdSun presets had something to clean.
- **Readiness checks real files; slider families are never "Missing".**
  [readiness.rs](src-tauri/src/readiness.rs) indexes the active setup once:
  MO2 `modlist.txt` (separators skipped), `plugins.txt` (`*` = active;
  official masters and `Skyrim.ccc` count as active), the overwrite folder,
  `Data`, and every mod root's plugins and BSAs. Loose textures are found by
  a walk that only descends folders a wanted file sits under, so 1,900+ mod
  folders take milliseconds. A BSA loads when `Skyrim.ini`
  (`sResourceArchiveList`/`2`, the profile's own INI under MO2) names it or
  an active plugin claims it (`X.bsa`, `X - *.bsa`); archives are only read
  for textures still unfound. [bsa.rs](src-tauri/src/bsa.rs) reads name
  tables only (v103–105). Slider families match the library's Nexus id
  against `meta.ini` modids, and since any mod can provide a family, "no
  enabled mod from that page" is Unconfirmed, never Missing. On the real
  setup ECE's CME_/ECE_ families are unconfirmed in 1,762 of 1,817 presets
  (12302 isn't installed and no tri, script or plugin names those sliders),
  so the sweep's "ready" means nothing Missing and unconfirmed is counted
  apart. First real sweep: 854 ready, 963 missing something, 2.6 s; the
  big causes are presets naming other versions' plugins
  (`MikanEyes All in one SE.esp`, `Kyoe_BanginBrows.esp` against the
  installed `Kyoe BanginBrows.esp`). The e2e test brute-forces every
  reported-missing reference against enabled mods, so a false alarm fails.
- **Precedence rule: user > automatic > seed.** A manual library entry
  (kind, match_type, pattern — the "shadow key") always wins over a seed
  entry covering the same pattern; the shadowed seed entry is marked
  `enabled: false` rather than deleted, so restoring it later needs no data
  recovery. A seed entry can also be disabled outright without a
  replacement (`disabled_seed_ids`). Among entries that remain enabled,
  `Library::match_entry` ranks by specificity first — exact beats prefix,
  a longer prefix beats a shorter one — and only falls back to
  user-over-seed as the final tiebreak at equal specificity. "Automatic"
  here means the existing Nexus MD5/mod-id resolution path in
  [assets.rs](src-tauri/src/assets.rs); the library only fills in what that
  path leaves unresolved, and a user library entry can still override an
  automatic Nexus match for the same reference.
- **`page_url` rename.** `IdentifiedGroup.nexus_url` (the original,
  Nexus-only field name) became `page_url` once library entries could
  resolve to non-Nexus links — the seed's High Poly Head entry, for
  example, points at vectorplexus.com. The Find Assets page's link button
  now reads "Open on Nexus" only when `page_url` contains
  `nexusmods.com`, and "Open page" otherwise.
- **Seed promotion pipeline.** The seed list ships as a curated best-effort
  draft, not a finished catalog. After
  Heath reviews and corrects entries in-app (Collection Review → Asset
  Library screen), his `library.json` corrections are meant to be folded
  back into `seed-library.json` ahead of a release: `user-*` ids
  re-slugged to `seed-<slug>`, duplicates against existing seed patterns
  resolved by keeping his correction, then the seed validation test
  (`library::tests::seed_file_is_valid_and_unique` — checks id/shadow-key
  uniqueness and runs every entry through the same `validate()` the UI
  uses) rerun before the next build. This keeps corrections that would
  otherwise live only on his machine available to every install.
