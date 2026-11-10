#!/usr/bin/env bash
# Download a short clip from the middle of each video.
# Usage: clip-middle.sh [-l seconds] [-o outdir] [-r height] [-a] URL [URL...]
#   -l  clip length in seconds (default 10)
#   -o  output directory (default ./clips)
#   -r  max video height, e.g. 720 (default best)
#   -a  audio only (m4a)
set -euo pipefail

len=10 out=./clips audio=0 h=""
while getopts "l:o:r:a" opt; do
  case $opt in
    l) len=$OPTARG ;; o) out=$OPTARG ;; r) h="[height<=$OPTARG]" ;; a) audio=1 ;;
    *) echo "usage: $0 [-l seconds] [-o outdir] [-r height] [-a] URL..." >&2; exit 2 ;;
  esac
done
shift $((OPTIND - 1))
[ $# -gt 0 ] || { echo "usage: $0 [-l seconds] [-o outdir] [-r height] [-a] URL..." >&2; exit 2; }
mkdir -p "$out"

if [ "$audio" = 1 ]; then fmt=(-f "ba[ext=m4a]/ba" -x --audio-format m4a)
else fmt=(-f "bv*${h}[ext=mp4]+ba[ext=m4a]/b${h}[ext=mp4]/bv*${h}+ba/b${h}" --merge-output-format mp4); fi

fail=0
for url in "$@"; do
  dur=$(yt-dlp --no-warnings --no-playlist --print duration "$url" 2>/dev/null | head -1) || dur=""
  if ! [[ $dur =~ ^[0-9.]+$ ]]; then echo "skip (no duration): $url" >&2; fail=1; continue; fi
  read -r start end < <(awk -v d="$dur" -v l="$len" 'BEGIN{
    if (d <= l) { s = 0; e = d } else { s = (d - l) / 2; e = s + l }
    printf "%.2f %.2f\n", s, e }')
  echo "$url: ${dur}s long, clipping ${start}-${end}"
  yt-dlp --no-warnings --no-playlist "${fmt[@]}" \
    --download-sections "*${start}-${end}" --force-keyframes-at-cuts \
    -o "$out/%(title).80B [%(id)s] mid.%(ext)s" "$url" || { echo "failed: $url" >&2; fail=1; }
done
exit $fail
