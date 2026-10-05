#!/bin/bash

# Let Omarchy's idle service and menu start either Omarchy's screensaver or
# tan-screensaver, whichever tan-screensaver-select chose (Omarchy's by default;
# SPEC §5). Adds Style > Screensaver > Omarchy Text / Tan Trajectories to the menu to
# choose, and System > Screensaver starts the chosen one.
#
# omarchy-launch-screensaver is called by name from the idle service and comes first in
# PATH, so it cannot be shadowed. Instead the idle plugin is cloned as a user plugin,
# with the launch command changed, and enabled (which disables the built-in one in
# shell.json). Re-run after an Omarchy update to regenerate the clone from the new upstream
# service. `--remove` restores the built-in service and menu entries.
#
# shell.json, the menu extension file and any previous clone are backed up first, to
# ~/.local/state/tan-screensaver/backups/<time>/.

set -euo pipefail

upstream=/usr/share/omarchy/shell/plugins/services/idle
upstream_command=omarchy-launch-screensaver
our_command=tan-screensaver-launch
plugin_id=${USER:-$(id -un)}.idle
plugin_dir=$HOME/.config/omarchy/plugins/$plugin_id
shell_json=$HOME/.config/omarchy/shell.json
menu=$HOME/.config/omarchy/extensions/omarchy-menu.jsonc
# The menu parser drops only whole-line comments, and a parse error makes it ignore the
# whole extension file, so the entries sit between marker lines rather than carrying a
# trailing comment.
begin='// tan-screensaver begin'
end='// tan-screensaver end'
state=${XDG_STATE_HOME:-$HOME/.local/state}/tan-screensaver
backups=$state/backups/$(date +%Y%m%d-%H%M%S)

fail() {
  echo "install-omarchy-hook: $*" >&2
  exit 1
}

backup() {
  [[ -e $1 ]] || return 0
  mkdir -p "$backups"
  cp -a "$1" "$backups/"
}

# The clone, as `omarchy plugin clone` makes it, but written here: that command deletes
# its clone when the shell is slow to answer, even though the shell has already switched
# shell.json over to it, which leaves no idle service at all. Code files are written
# next to their targets and renamed, so the shell never reloads a half-written file.
write_clone() {
  local dir=$1 file tmp
  jq --arg id "$plugin_id" '
      .id = $id | .name = "My Idle" |
      .omarchy = ((if (.omarchy | type) == "object" then .omarchy else {} end)
        + { clonedFrom: "omarchy.idle" })
    ' "$upstream/manifest.json" >"$dir/manifest.json.tmp"
  mv -f "$dir/manifest.json.tmp" "$dir/manifest.json"
  for file in "$upstream"/*; do
    [[ ${file##*/} == manifest.json ]] && continue
    tmp=$(mktemp "$dir/.${file##*/}.XXXXXX")
    if [[ ${file##*/} == Service.qml ]]; then
      sed "s|$upstream_command|$our_command|" "$file" >"$tmp"
    else
      cp "$file" "$tmp"
    fi
    chmod 644 "$tmp"
    mv -f "$tmp" "$dir/${file##*/}"
  done
}

idle_disabled() {
  jq -e '(.disabledPlugins // []) | index("omarchy.idle") != null' "$shell_json" >/dev/null 2>&1
}

remove() {
  backup "$shell_json"
  backup "$menu"
  backup "$plugin_dir"
  # Enabling the built-in drops the clone from shell.json and re-enables the original.
  if idle_disabled; then
    omarchy plugin enable omarchy.idle
  fi
  rm -rf "$plugin_dir"
  omarchy-toggle tan-screensaver off
  if [[ -f $menu ]]; then
    sed -i "\|^ *$begin\$|,\|^ *$end\$|d" "$menu"
  fi
  omarchy-shell shell rescanPlugins >/dev/null || true
  echo "Restored Omarchy's idle service and menu entry. Backups: $backups"
}

install() {
  command -v "$our_command" >/dev/null && command -v tan-screensaver-select >/dev/null ||
    fail "$our_command is not in PATH; run scripts/install.sh first"
  [[ -f $upstream/Service.qml && -f $upstream/manifest.json ]] || fail "no idle service at $upstream"
  local uses
  uses=$(grep -c "$upstream_command" "$upstream/Service.qml" || true)
  [[ $uses == 1 ]] || fail "expected one call to $upstream_command in $upstream/Service.qml, found $uses; the upstream service changed, update this script"

  backup "$shell_json"
  backup "$menu"
  backup "$plugin_dir"

  if [[ -d $plugin_dir ]]; then
    write_clone "$plugin_dir"
  else
    local stage
    stage=$(mktemp -d "${plugin_dir%/*}/.clone.XXXXXX")
    write_clone "$stage"
    mv "$stage" "$plugin_dir"
  fi
  omarchy-shell shell rescanPlugins >/dev/null || true

  if ! idle_disabled; then
    # The shell may report "not responding" after making the change; shell.json is
    # what counts.
    omarchy plugin enable "$plugin_id" || true
    for _ in {1..50}; do
      idle_disabled && break
      sleep 0.1
    done
  fi
  idle_disabled || fail "omarchy.idle is still enabled in $shell_json"
  grep -q "$our_command" "$plugin_dir/Service.qml" || fail "the clone does not call $our_command"

  # Reusing the menu's id overrides only the fields given.
  if [[ ! -f $menu ]]; then
    mkdir -p "${menu%/*}"
    printf '{\n}\n' >"$menu"
  fi
  # Rewritten on every run, so a re-run picks up changes to these entries. Each entry
  # gives all its fields: the parser fills in missing ones (the label becomes the id),
  # so an entry overriding a default one replaces it whole.
  sed -i "\|^ *$begin\$|,\|^ *$end\$|d" "$menu"
  local chosen='omarchy-toggle-enabled tan-screensaver'
  local lines=(
    "$begin"
    "\"system.screensaver\": {\"icon\":\"󱄄\",\"label\":\"Screensaver\",\"action\":\"$our_command force\"},"
    "\"style.screensaver.omarchy\": {\"icon\":\"󱄄\",\"label\":\"Omarchy Text\",\"checked\":\"! $chosen\",\"action\":\"tan-screensaver-select omarchy\"},"
    "\"style.screensaver.tan\": {\"icon\":\"󱄄\",\"label\":\"Tan Trajectories\",\"checked\":\"$chosen\",\"action\":\"tan-screensaver-select tan\"},"
    "$end"
  )
  local line last
  for line in "${lines[@]}"; do
    # Before the closing brace of the top-level object.
    last=$(grep -n '^}' "$menu" | tail -1 | cut -d: -f1)
    [[ -n $last ]] || fail "cannot find the closing brace in $menu"
    sed -i "${last}i\\  $line" "$menu"
  done

  echo "Idle service: $plugin_id runs $our_command (omarchy.idle disabled)"
  echo "Menu: Style > Screensaver chooses; System > Screensaver starts the chosen one"
  echo "Current choice: $(tan-screensaver-select) (change with tan-screensaver-select tan|omarchy)"
  [[ -d $backups ]] && echo "Backups: $backups"
}

case ${1:-} in
"") install ;;
--remove) remove ;;
-h | --help) sed -n '3,/^$/{/^#/!q;p}' "$0" | sed 's/^# \{0,1\}//' ;;
*) fail "unknown option: $1 (expected --remove or nothing)" ;;
esac
