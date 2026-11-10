#!/bin/bash
# Copies the source tree from the Windows checkout (D:\Anthropic\PROJECTS\voidflare) into the WSL home, where the
# Linux build runs: building on ext4 is far faster than on /mnt/d. Run inside WSL. Only sources travel: targets,
# downloaded engines, release output and the Windows-only portable folder stay behind.
set -euo pipefail
SRC="${1:-/mnt/d/Anthropic/PROJECTS/voidflare}"
DEST="${2:-$HOME/swapcraft}"
mkdir -p "$DEST"
rsync -a --delete \
  --exclude '/src/target/' --exclude '/src/engines/' --exclude '/release/' --exclude '/portable/' --exclude '/portable-linux/' --exclude '/legacy/' \
  --exclude '/.git/' --exclude '/src/linux-out/' \
  "$SRC/" "$DEST/"
# Windows checkouts may carry CRLF line ends in the shell scripts
find "$DEST/src" -maxdepth 1 -name '*.sh' -exec sed -i 's/\r$//' {} +
find "$DEST/src/tools/linux" -name '*.sh' -exec sed -i 's/\r$//' {} +
echo "synced to $DEST"
