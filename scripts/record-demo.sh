#!/usr/bin/env bash
# Record the README's GIF (docs/src/demo.gif) from the demo in docs/demo: a
# scripted model, and keys played into the window. macOS only: it needs
# `screencapture` (with Screen Recording allowed for the terminal) and ffmpeg.
set -euo pipefail
cd "$(dirname "$0")/.."

out=${1:-docs/src/demo.gif}
work=$(mktemp -d)
# The project somewhere short, as the terminal shows its path.
project=/tmp/shop
trap 'rm -rf "$work" "$project"' EXIT

cargo build --features demo
rm -rf "$project"
cp -R docs/demo/project "$project"
# A plain prompt in the terminal, and none of this machine's own state.
mkdir -p "$work/zsh" "$work/support"
printf "PROMPT='%%1~ %%# '\nunsetopt PROMPT_SP\n" > "$work/zsh/.zshrc"

GHOSTWRITE_DEMO="$PWD/docs/demo" GHOSTWRITE_SUPPORT_DIR="$work/support" ZDOTDIR="$work/zsh" SHELL=/bin/zsh PYTHONDONTWRITEBYTECODE=1 \
    target/debug/ghostwrite "$project" "$project/cart.py" 2> "$work/log" &
app=$!

# The demo says where its window is, then waits a moment before the first key.
until grep -q '^demo window' "$work/log"; do sleep 0.1; done
read -r _ _ x y w h < <(grep '^demo window' "$work/log")
region=$(printf '%.0f,%.0f,%.0f,%.0f' "$x" "$y" "$w" "$h")
screencapture -v -R"$region" "$work/demo.mov" &
capture=$!
wait "$app" || true
kill -INT "$capture" 2> /dev/null || true
wait "$capture" || true

# 12 frames a second, 960 wide, with a palette made for it.
ffmpeg -loglevel error -y -i "$work/demo.mov" \
    -vf "fps=12,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4" \
    "$out"
echo "wrote $out ($(du -h "$out" | cut -f1))"
