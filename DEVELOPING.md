# Developing obelisk-arena

## New machine
1. SSH key authorized for github.com/LukeThayer, **and with access to the private vothuul/obelisk repo** (the stat/loot rules library is an ssh git dependency — you need the key to *build*, not just push).
2. `git clone git@github.com:LukeThayer/obelisk-arena.git && cd obelisk-arena`
3. `nix develop` (Linux needs nix with flakes enabled; macOS also works — or skip nix and use a rustup toolchain ≥1.93 with plain cargo)
4. `cargo build && cargo test` — the game workspace.
5. `cd crates/arena_editor && cargo build` — the editor is its OWN cargo workspace (never `-p arena_editor` from the root); run with `cargo run --bin arena-editor`, press `K` for Skill mode.

All external deps are git dependencies (vothuul/obelisk, LukeThayer/bevy-obelisk, LukeThayer/bevy_modal_editor) pinned by the committed Cargo.locks — no sibling checkouts needed to build.

## Co-developing the libraries
`tools/dev-siblings.sh` clones the three library repos as `../` siblings and writes **git-ignored**
`.cargo/config.toml` `[patch]` files (in this repo, in `crates/arena_editor/`, and in `../obelisk-bevy/`)
redirecting the git deps to the local checkouts. Edit libs + game together with instant rebuilds;
`tools/dev-siblings.sh --off` returns to pure git deps.

**Sync point** (publishing lib changes): commit+push the lib, then in consumers run
`cargo update -p <crate>` with patches OFF and commit the lock. Never commit a Cargo.lock generated
with patches ON (`git diff Cargo.lock` will show `path+` sources if you did).

## Verification
- Golden combat traces (in `../obelisk-bevy`): `cargo test --features test-support --test golden` — must be byte-identical; never blind-`UPDATE_GOLDEN`.
- Net-test: `pkill -f arena-server; pkill -f arena-client; sleep 1; bash crates/arena_game/tools/net-test/run_session.sh` (flaky on wall-clock: retry ≤3×, one `session PASS` = green).
- Editor suite: `cd crates/arena_editor && cargo test`.

## Pinned things (do not "fix")
- `bevy_egui` rev `81904da…` everywhere — its `main` moved to Bevy 0.19 / rustc 1.95; this rev is the last Bevy-0.18-compatible one.
- `avian3d` 0.5 — pinned by `lightyear_avian3d` 0.26.
- `crates/arena_editor/Cargo.lock` (the Bevy 0.18 set) — no blanket `cargo update` in that workspace.
- The 12 `stat_core` dead-code warnings in test output are upstream's and allowed.
