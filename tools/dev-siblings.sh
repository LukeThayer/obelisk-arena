#!/usr/bin/env bash
# Co-dev escape hatch: redirect the git deps to ../ sibling checkouts via config-level [patch]
# (git-ignored .cargo/config.toml — daily edit-lib-rebuild-game loop without push/update ceremony).
#   tools/dev-siblings.sh        clone-or-update siblings + write patch configs
#   tools/dev-siblings.sh --off  remove patch configs (back to pure git deps)
# Lockfile convention: commit Cargo.locks only with patches OFF (sync points).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; SRC="$(dirname "$ROOT")"
CONFIGS=("$ROOT/.cargo/config.toml" "$ROOT/crates/arena_editor/.cargo/config.toml" "$SRC/obelisk-bevy/.cargo/config.toml")

if [[ "${1:-}" == "--off" ]]; then rm -fv "${CONFIGS[@]}"; exit 0; fi

clone_if_missing() { # name url
  if [[ -d "$SRC/$1/.git" ]]; then echo "sibling $1: present"; else git clone "$2" "$SRC/$1"; fi
}
clone_if_missing obelisk           "git@github.com:vothuul/obelisk.git"
clone_if_missing obelisk-bevy      "git@github.com:LukeThayer/bevy-obelisk.git"
clone_if_missing bevy_modal_editor "git@github.com:LukeThayer/bevy_modal_editor.git"

# Cargo resolves config-level [patch] relative paths against the directory holding .cargo/.
obelisk_patch() { local rel="$1"; cat <<EOF
[patch."ssh://git@github.com/vothuul/obelisk.git"]
stat_core = { path = "$rel/obelisk/stat_core" }
loot_core = { path = "$rel/obelisk/loot_core" }
skill_tree = { path = "$rel/obelisk/skill_tree" }
tables_core = { path = "$rel/obelisk/tables_core" }
EOF
}
mkdir -p "$ROOT/.cargo" "$ROOT/crates/arena_editor/.cargo" "$SRC/obelisk-bevy/.cargo"

{ obelisk_patch ".."; cat <<'EOF'
[patch."https://github.com/LukeThayer/bevy-obelisk"]
obelisk-bevy = { path = "../obelisk-bevy" }
[patch."https://github.com/LukeThayer/bevy_modal_editor"]
bevy_vfx = { path = "../bevy_modal_editor/crates/bevy_vfx" }
EOF
} > "$ROOT/.cargo/config.toml"

{ obelisk_patch "../../.."; cat <<'EOF'
[patch."https://github.com/LukeThayer/bevy-obelisk"]
obelisk-bevy = { path = "../../../obelisk-bevy" }
[patch."https://github.com/LukeThayer/bevy_modal_editor"]
bevy_modal_editor = { path = "../../../bevy_modal_editor" }
bevy_editor_game = { path = "../../../bevy_modal_editor/crates/bevy_editor_game" }
bevy_vfx = { path = "../../../bevy_modal_editor/crates/bevy_vfx" }
EOF
} > "$ROOT/crates/arena_editor/.cargo/config.toml"

obelisk_patch ".." > "$SRC/obelisk-bevy/.cargo/config.toml"
echo "patch configs written (tools/dev-siblings.sh --off to remove)"
