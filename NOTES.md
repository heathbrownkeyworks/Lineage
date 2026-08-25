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

- **`bodyMorphs`** is the section Remove BodySlide strips:
  `[{keys: [{key: "XPMSE.esp", value: n}], name: "MorphName"}]`. It includes
  BodySlide sliders *and* XPMSE skeleton morphs — both are body morph data
  RaceMenu re-applies on preset load. The candidate-key list is the
  `BODY_MORPH_KEYS` constant in [jslot.rs](src-tauri/src/jslot.rs); only
  `bodyMorphs` is confirmed in the wild so far.

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
- **Texture attribution** — a texture path that resolves to no loose file
  anywhere but starts with `actors\character\` is classified "base game"
  (vanilla BSAs aren't parsed); anything else unresolved goes to Unknown.
- **Morph names** never resolve to files; they're always listed under Unknown
  with a best-guess origin hint for recognizable prefixes (`EFM_`,
  `ECE_`/`CME_`, `XPMSE*`).
