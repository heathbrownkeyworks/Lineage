# Lineage

A RaceMenu `.jslot` preset utility by **ColdSun Creative**.

- **Find Assets** — pick a preset and trace every plugin, texture, and morph
  it references back to the mod it came from, with Nexus Mods matches, so you
  know exactly what to download to make a preset work.
- **Backup** — one action that archives every JSLOT file across your setup
  into a dated zip, folder structure preserved.
- **Clean Preset** — strip what a preset author's own setup left in a JSLOT
  (BodySlide sliders, body tattoos, skeleton and height scaling, weapon and
  camera placement), so a preset applies the face without overwriting your
  character. Face overlays are never touched.
- **Batch Clean** — the same cleanup across your whole collection, with
  per-file selection, automatic pre-change snapshots, and one-click restore.
- **Restore** — compare any backup with what's on disk and put back what
  changed; a launch nudge when presets aren't covered by your last backup.
- **Asset Library** — an editable list of preset-reference-to-mod mappings
  (plugins, textures, morph name prefixes) that Find Assets and Collection
  Review fall back on when Nexus can't resolve a reference. Ships with a
  curated seed list; every correction or addition you make is saved to your
  own `library.json` and takes precedence over the built-in entries.
- **Release** — for preset authors: the Nexus Requirements section for a
  pack (every mod its presets need, counted and linked, in Nexus BBCode,
  Markdown or plain text) and the zip players install, with the presets
  cleaned the same way and each preset's head export beside it.

Lineage never writes anything into your mod folders except the modified
preset itself — no `.bak` files, no stray temp files. Every destructive
operation snapshots the exact files it touches into your backup folder first
and can be undone from **History** (Settings → History).

## Requirements

- Windows 10/11
- [Rust](https://rustup.rs/) (stable) and the MSVC build tools
- [Node.js](https://nodejs.org/) 20+ and [pnpm](https://pnpm.io/)
- WebView2 runtime (preinstalled on Windows 11)

## Development

```bash
pnpm install
pnpm tauri dev
```

## Build

```bash
pnpm tauri build
```

The installer lands in `src-tauri/target/release/bundle/`.

Backend tests (including a byte-perfect round-trip check against real preset
files, skipped on machines without the corpus):

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

## Nexus Mods API key

Find Assets can identify mods via the Nexus Mods API. It needs your
**personal API key** from
[nexusmods.com/settings/api-keys](https://www.nexusmods.com/settings/api-keys)
(Site Preferences → API Keys). Paste it in **Settings → Nexus Mods API key**
and hit Validate.

Without a key, Find Assets still parses presets and lists every referenced
asset and plugin — it just can't match them to Nexus mod pages. Responses are
cached on disk so repeated scans don't burn your daily rate limit; the
remaining quota is shown after each scan.

The key is stored in Lineage's local config file, is never logged, and never
appears in error messages or the UI once saved.

## Where things live

| Thing | Location |
|---|---|
| Settings | `%APPDATA%\com.coldsun.lineage\settings.json` |
| Asset Library (your entries) | `%APPDATA%\com.coldsun.lineage\library.json` |
| Nexus response cache | `%APPDATA%\com.coldsun.lineage\cache\nexus\` |
| Full backups | your configured backup folder (default `Documents\Lineage\Backups`) |
| Operation snapshots | `<backup folder>\Snapshots\` |

Full backups are never deleted by Lineage. Snapshots are pruned automatically
past the configured retention (default 10).

## Design & structure

UI is built on the **Starfall Design System** (vendored tokens at
`src/lib/starfall/tokens.css`); the application shell follows **Visage**'s
structure. See [NOTES.md](NOTES.md) for what was reused and every format
confirmed against real data (JSLOT dialects, MO2/Vortex metadata, Nexus
endpoints).
