# 🎵 merge_media

A simple GUI tool for merging external audio tracks into video files using `ffmpeg` and `ffprobe`, built with Rust (`eframe`/`egui`).

## ✨ Key Features

- **Auto-detects audio streams** in external media
- **Bundled `ffmpeg`/`ffprobe`** — no separate install needed
- **Intuitive GUI** powered by `egui`
- **Select audio tracks by language**
- **Log output** to file and UI, with toggleable visibility
- **Windows admin relaunch** for elevated operations

## 🖼 Interface Overview

The main window allows you to:

1. Select a **video file** (input)
2. Select an **external audio file** (e.g., dubbed track)
3. Pick the desired audio track
4. Set the **output file** path
5. View logs and status in a toggleable log panel

## 🏗 Architecture

### `AudioMergerApp` Structure

Manages all application state:

- File paths: `video_path`, `audio_path`, `output_path`
- Audio tracks: `audio_tracks` (`(stream_index, language_code)`)
- Selection: `selected_track`
- Logging: `logs`
- Communication: `tx`, `rx`
- UI state: `is_merging`, `show_logs`
- Executable directory: `exe_dir` (for ffmpeg/ffprobe)

## 🔧 Core Functions

- `main()` — app entry point
- `relaunch_as_admin()` — relaunches with admin rights (Windows)
- `widestring()` — converts strings to Windows wide strings
- `is_elevated()` — checks for admin privileges
- `get_all_audio_tracks()` — parses audio streams via `ffprobe`
- `find_audio_track()` — finds audio stream by language code

## 🧪 Type Aliases

```rust
type AudioStream = (u32, String);
```

## 🚀 Getting Started

```bash
git clone https://github.com/yourusername/merge_media.git
cd merge_media
cargo build --release
```

Place `ffmpeg.exe` and `ffprobe.exe` in the executable directory or where the app expects (`exe_dir`).

## 💻 Platform Support

- **Windows**: Full support (admin relaunch logic)
- **Other platforms**: Adaptable with minor changes

---

> _Easily merge dubbed audio into videos with a streamlined, user-friendly interface._
