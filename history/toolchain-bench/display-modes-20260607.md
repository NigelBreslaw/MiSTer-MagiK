# Display Mode Sweep - 2026-06-07

PR5/PR6 visual verification record for dynamic benchmark display modes.

## HDMI / framebuffer modes

| Label | INI mode | Detected fb | Render | Scale | Visual result |
|-------|----------|-------------|--------|-------|---------------|
| auto | `[Menu] video_mode` commented | 1920x1080 | 960x540 | 2 | Stable; MiSTer selected 1080p by EDID/native detection |
| low | preset `6` | 640x480 | 640x480 | 1 | Stable |
| 960 | `960,540,60` | 960x540 | 960x540 | 1 | Stable |
| 720p | preset `0` | 1280x720 | 1280x720 | 1 | Stable |
| 1080p | preset `8` | 1920x1080 | 960x540 | 2 | Stable |
| high | preset `14` | 1280x720 reported by fb path | 1280x720 | 1 | Failed on this TV: glitchy, top half only |

Important finding: `video_mode=1280,720,60` is not equivalent to MiSTer preset
`0`; it asks MiSTer to synthesize a CVT-RB mode. On this TV that made stock
MiSTer, games, and the Slint framebuffer jump badly. Use preset IDs for standard
HDMI sweep labels.

## 1440p

Stock MiSTer with preset `14` and stock MiSTer with calculated
`2560,1440,60` were both glitchy on this TV. Stock EDID/native mode was stable
but selected 1920x1080, not 1440p. Treat forced 1440p as display-dependent.

## CRT / direct-video smoke

| Label | Owner | INI keys | Detected fb | TV report | Visual result |
|-------|-------|----------|-------------|-----------|---------------|
| direct-auto | stock MiSTer | `direct_video=2`, `menu_pal=0`, `forced_scandoubler=0` | 1920x1080 | 1080p path | Stable; not detected as known DAC |
| ntsc31 | stock MiSTer | `direct_video=1`, `menu_pal=0`, `forced_scandoubler=1` | 640x480 | 529x480p | Stable; TV-managed aspect ratio |

15 kHz CRT output was not visually tested in this run.
