#!/usr/bin/env bash
# Screenshots every TUI view against an existing index with vhs.
#
#   scripts/screenshots.sh DATA_DIR OUT_DIR [large|small]
#
# DATA_DIR holds index.db (it is opened read-write, so pass a copy you can
# spare). `large` is 2000x1250 px at font 16 (about 200x50 cells); `small`
# is the 80x24 equivalent. Trash, state, and config go to OUT_DIR/env, so a
# run never touches ~/.Trash or your real config.
set -euo pipefail

data=$(cd "$1" && pwd)
mkdir -p "$2"
out=$(cd "$2" && pwd)
size=${3:-large}
repo=$(cd "$(dirname "$0")/.." && pwd)
bin="$repo/target/debug/spacehog"
cargo build -q -p spacehog --manifest-path "$repo/Cargo.toml"

case "$size" in
  large) width=2000; height=1250; font=16 ;;
  small) width=740; height=520; font=14 ;;
  *) echo "size must be large or small" >&2; exit 2 ;;
esac

env_dir="$out/env"
mkdir -p "$env_dir/trash" "$env_dir/state" "$env_dir/config"
if [[ -n "${THEME:-}" ]]; then
  printf 'theme = "%s"\n' "$THEME" > "$env_dir/config/config.toml"
fi

tape="$out/shots-$size.tape"
cat > "$tape" <<EOF
Set Shell "bash"
Set FontSize $font
Set Width $width
Set Height $height
Set Padding 10
Set Theme { "name": "spacehog", "black": "#121417", "red": "#dc8a90", "green": "#7cd591", "yellow": "#f2b966", "blue": "#87bafd", "magenta": "#c68bd3", "cyan": "#65d0dc", "white": "#e3e5e8", "brightBlack": "#82868e", "brightRed": "#dc8a90", "brightGreen": "#7cd591", "brightYellow": "#f0c374", "brightBlue": "#92c1fd", "brightMagenta": "#eaa4d2", "brightCyan": "#a8d8fb", "brightWhite": "#ffffff", "background": "#121417", "foreground": "#e3e5e8", "selection": "#282c31", "cursor": "#92c1fd" }
Set WaitTimeout 300s
Env SPACEHOG_DATA_DIR "$data"
Env SPACEHOG_CONFIG_DIR "$env_dir/config"
Env SPACEHOG_STATE_DIR "$env_dir/state"
Env SPACEHOG_TRASH_DIR "$env_dir/trash"
Env COLORTERM "truecolor"
Env NO_COLOR "${NO_COLOR:-}"
Hide
Type "clear && $bin ${SPACEHOG_ARGS:-}"
Enter
Wait+Screen /Overview/
Sleep 3s
Show
Screenshot $size-1-overview.png
Sleep 1s
Type "2"
Sleep 1s
Down
Down
Type "p"
Sleep 1s
Screenshot $size-2-explorer.png
Sleep 1s
Type "3"
Sleep 2s
Screenshot $size-3-treemap.png
Sleep 1s
Type "4"
Sleep 2s
Down
Enter
Sleep 1s
Type " "
Down
Down
Type " "
Sleep 500ms
Screenshot $size-4-categories.png
Sleep 1s
Type "d"
Sleep 1s
Type "5"
Sleep 1s
Screenshot $size-5-cleanup.png
Sleep 1s
Right
Sleep 1s
Screenshot $size-5-proposals.png
Sleep 1s
Type "6"
Sleep 3s
Screenshot $size-6-changes.png
Sleep 1s
Type "1"
Type "q"
EOF
cd "$out"
vhs "$(basename "$tape")" >/dev/null
ls "$out"/"$size"-*.png
