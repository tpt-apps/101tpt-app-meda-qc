# Encoded fixture attribution

`encoded/vp9-clip.mp4` and `encoded/vp9-clip.webm` are royalty-free VP9 clips
generated locally with FFmpeg's `libvpx-vp9` encoder (three 64x64 `testsrc`
frames, `-fflags +bitexact -map_metadata -1` so the files are reproducible):

```text
ffmpeg -f lavfi -i "testsrc=size=64x64:rate=10:duration=0.3" -pix_fmt yuv420p \
  -frames:v 3 -c:v libvpx-vp9 -b:v 40k -deadline good -cpu-used 8 -g 30 \
  -fflags +bitexact -flags:v +bitexact -map_metadata -1 -f mp4  encoded/vp9-clip.mp4
ffmpeg -f lavfi -i "testsrc=size=64x64:rate=10:duration=0.3" -pix_fmt yuv420p \
  -frames:v 3 -c:v libvpx-vp9 -b:v 40k -deadline good -cpu-used 8 -g 30 \
  -fflags +bitexact -flags:v +bitexact -map_metadata -1 -f webm encoded/vp9-clip.webm
```

libvpx is © Google and distributed under the BSD-3-Clause licence; FFmpeg is
© the FFmpeg developers and licensed under LGPL-2.1-or-later/GPL-2.0-or-later.
The clips are synthetic `testsrc` patterns with no third-party content, and are
used only to exercise the pinned Kinetix VP9 decoder through the MP4 and
Matroska/WebM demuxers.

AV1 decode coverage needs no encoded fixture: the decode tests synthesise
frames and encode them with `tpt-kinetix-av1`'s `Av1Encoder` (rav1e, BSD-2-Clause
Patents / Apache-2.0 dual licence) at test time, then wrap the packets in a
minimal Matroska file. H.264 encoded fixtures were removed when the H.264
decoder was dropped — no AVC code is exercised or shipped.
