#![windows_subsystem = "windows"]
//! Audio Merger GUI (maximized window, toggleable logs, true no‐console ffmpeg/ffprobe).
//!
//! 1. Auto‐elevate via CheckTokenMembership + “--elevated” guard (only once).  
//! 2. Window opens maximized (not kiosk fullscreen).  
//! 3. Logs are hidden by default; click “Show Logs” to expand a scroll area.  
//! 4. ffprobe/ffmpeg are spawned with CREATE_NO_WINDOW so that no console pops up.  
//! 5. Uses `-hide_banner -loglevel error -y` for silent, overwrite‐without‐prompt operation.

use eframe::{egui, run_native, App, Frame, NativeOptions};
use log::info;
use rfd::FileDialog;
use serde_json::Value;
use simplelog::{CombinedLogger, ConfigBuilder, WriteLogger};
use std::ffi::OsStr;
use std::fs::File;
use std::os::windows::ffi::OsStrExt; // for OsStrExt::encode_wide()
use std::os::windows::process::CommandExt; // for .creation_flags()
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

// ————— WinAPI imports from winapi = "0.3" ——————————————————————————————
use winapi::shared::minwindef::{DWORD, FALSE};
use winapi::shared::ntdef::NULL;
use winapi::um::handleapi::CloseHandle;
use winapi::um::processthreadsapi::{GetCurrentProcess, OpenProcessToken};
use winapi::um::securitybaseapi::{CheckTokenMembership, CreateWellKnownSid};
use winapi::um::shellapi::ShellExecuteW;
use winapi::um::winbase::LocalFree;
use winapi::um::winnt::{WinBuiltinAdministratorsSid, TOKEN_QUERY};
use winapi::um::winuser::SW_SHOWNORMAL;
// ———————————————————————————————————————————————————————————————————————

/// Ensures we run elevated exactly once (using “--elevated” flag to avoid loops).
fn ensure_admin() {
    // If "--elevated" is present, skip re‐launch.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|s| s == "--elevated") {
        return;
    }

    unsafe {
        // 1) Open process token with TOKEN_QUERY
        let mut token_handle = std::ptr::null_mut();
        let proc = GetCurrentProcess();
        let success = OpenProcessToken(proc, TOKEN_QUERY, &mut token_handle);
        if success == 0 {
            // Could not open token → assume not elevated, re‐launch
            do_relaunch();
            return;
        }

        // 2) Create a WELL_KNOWN_SID for BUILTIN\Administrators
        let mut sid_buffer = [0u8; 68];
        let mut sid_size: DWORD = sid_buffer.len() as DWORD;
        let sid_ptr = sid_buffer.as_mut_ptr() as *mut _;

        let created = CreateWellKnownSid(
            WinBuiltinAdministratorsSid,
            std::ptr::null_mut(), // no domain SID
            sid_ptr as *mut _,
            &mut sid_size,
        );
        if created == 0 {
            // Failed → close token handle, but do NOT re‐launch so as not to loop
            CloseHandle(token_handle);
            return;
        }

        // 3) CheckTokenMembership(token_handle, sid_ptr, &mut is_member)
        let mut is_member: i32 = 0; // BOOL is a 32-bit integer
        let checked = CheckTokenMembership(token_handle, sid_ptr as *mut _, &mut is_member);

        // Free the SID buffer if needed
        LocalFree(sid_ptr as *mut _);

        if checked == 0 || is_member == FALSE as i32 {
            // Not elevated → re‐launch with "--elevated"
            CloseHandle(token_handle);
            do_relaunch();
        } else {
            // Already elevated
            CloseHandle(token_handle);
        }
    }
}

/// Re‐launches this executable with the “runas” verb and “--elevated” parameter, then exits.
fn do_relaunch() {
    // Build wide string for "runas"
    let verb_w = wide_str("runas");
    // Build wide string for this EXE’s path
    let exe_path = std::env::current_exe().expect("Failed to get current exe path");
    let exe_wide: Vec<u16> = OsStr::new(&exe_path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // Parameter “--elevated”
    let param_wide: Vec<u16> = wide_str("--elevated");

    unsafe {
        ShellExecuteW(
            NULL as _,
            verb_w.as_ptr(),
            exe_wide.as_ptr(),
            param_wide.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
    std::process::exit(0);
}

/// Convert &str → nul-terminated wide (UTF-16) Vec<u16>.
fn wide_str(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// (stream_index, language_tag)
type AudioStream = (u32, String);

/// Application state
struct AudioMergerApp {
    video_path: Option<String>,
    audio_path: Option<String>,
    output_path: Option<String>,
    audio_tracks: Vec<AudioStream>,
    selected_audio_track: Option<u32>,
    logs: Vec<String>,
    rx: Receiver<String>,
    tx: Sender<String>,
    is_merging: bool,
    show_logs: bool, // ← whether to show the log pane
}

impl Default for AudioMergerApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            video_path: None,
            audio_path: None,
            output_path: None,
            audio_tracks: Vec::new(),
            selected_audio_track: None,
            logs: Vec::new(),
            rx,
            tx,
            is_merging: false,
            show_logs: false,
        }
    }
}

impl AudioMergerApp {
    /// Append a message to both the on‐screen log and the file logger.
    fn append_log(&mut self, message: &str) {
        self.logs.push(message.to_string());
        info!("{}", message);
    }

    /// Run ffprobe on `path`, fill `self.audio_tracks`, auto‐select “por” if found.
    fn probe_audio_tracks(&mut self, path: &str) {
        self.append_log(&format!("Probing audio streams in: {}", path));
        let tracks = get_all_audio_tracks(path, &|msg| {
            let _ = self.tx.send(msg.to_string());
        });
        self.audio_tracks = tracks;

        // Try auto‐select “por”
        self.selected_audio_track = self.audio_tracks.iter().find_map(|(idx, lang)| {
            if lang.eq_ignore_ascii_case("por") {
                Some(*idx)
            } else {
                None
            }
        });

        if let Some(idx) = self.selected_audio_track {
            self.append_log(&format!("Auto-selected Portuguese track index {}.", idx));
        } else if self.audio_tracks.is_empty() {
            self.append_log("Warning: No audio streams found in that file.");
        } else {
            self.append_log("No 'por' track found; please pick from dropdown.");
        }
    }

    /// Spawn a thread that calls ffprobe→ffmpeg using CREATE_NO_WINDOW so no console appears.
    fn start_merge(&mut self) {
        let video = match &self.video_path {
            Some(v) => v.clone(),
            None => {
                self.append_log("Error: Please select a video file first.");
                return;
            }
        };
        let audio = match &self.audio_path {
            Some(a) => a.clone(),
            None => {
                self.append_log("Error: Please select an audio file first.");
                return;
            }
        };
        let output = match &self.output_path {
            Some(o) => o.clone(),
            None => {
                self.append_log("Error: Please select an output path first.");
                return;
            }
        };
        let track = match self.selected_audio_track {
            Some(t) => t,
            None => {
                self.append_log("Error: Please pick an audio track from the dropdown.");
                return;
            }
        };

        if self.is_merging {
            self.append_log("Merge is already in progress.");
            return;
        }
        self.is_merging = true;
        self.append_log(&format!(
            "Starting merge with external audio index {}...",
            track
        ));

        let thread_tx = self.tx.clone();
        thread::spawn(move || {
            let log = |msg: &str| {
                let _ = thread_tx.send(msg.to_string());
            };

            // 1) Try to find “eng” in the video
            log("ffprobe → searching for 'eng' in video...");
            match find_audio_track(&video, "eng", &log) {
                Some(eng_idx) => {
                    log(&format!("Found 'eng' at index {}.", eng_idx));
                    log("Running ffmpeg to merge...");
                    let status = Command::new("ffmpeg")
                        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                        .args(&[
                            "-hide_banner",
                            "-loglevel",
                            "error",
                            "-y",
                            "-i",
                            &video,
                            "-i",
                            &audio,
                            "-map",
                            "0:0", // video stream
                            "-map",
                            &format!("0:{}", eng_idx), // video audio “eng”
                            "-map",
                            &format!("1:{}", track), // external track
                            "-c",
                            "copy",
                            &output,
                        ])
                        .status();

                    if let Ok(s) = status {
                        if s.success() {
                            log("Merge completed successfully!");
                        } else {
                            log(&format!("ffmpeg exited with code {:?}.", s.code()));
                        }
                    } else {
                        log("Error: Unable to run ffmpeg.exe.");
                    }
                }
                None => {
                    log("No 'eng' in video; using fallback audio 0:1 from video.");
                    log("Running ffmpeg to merge...");
                    let status = Command::new("ffmpeg")
                        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                        .args(&[
                            "-hide_banner",
                            "-loglevel",
                            "error",
                            "-y",
                            "-i",
                            &video,
                            "-i",
                            &audio,
                            "-map",
                            "0:0", // video stream
                            "-map",
                            "0:1", // fallback audio
                            "-map",
                            &format!("1:{}", track), // external track
                            "-c",
                            "copy",
                            &output,
                        ])
                        .status();

                    if let Ok(s) = status {
                        if s.success() {
                            log("Merge completed successfully (fallback video audio)!");
                        } else {
                            log(&format!("ffmpeg exited with code {:?}.", s.code()));
                        }
                    } else {
                        log("Error: Unable to run ffmpeg.exe.");
                    }
                }
            }

            let _ = thread_tx.send("MERGE_DONE".to_string());
        });
    }
}

impl App for AudioMergerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        // 1) Drain any messages from the background thread
        while let Ok(msg) = self.rx.try_recv() {
            if msg == "MERGE_DONE" {
                self.append_log("Merge thread finished.");
                self.is_merging = false;
            } else {
                self.append_log(&msg);
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Audio Merger GUI");
            ui.separator();

            // — Select Video File —
            ui.horizontal(|ui| {
                if ui.button("Select Video File").clicked() {
                    if let Some(p) = FileDialog::new()
                        .add_filter("Video", &["mp4", "mkv", "avi", "mov", "flv", "wmv"])
                        .set_title("Pick video file")
                        .pick_file()
                    {
                        if let Some(s) = p.to_str() {
                            self.video_path = Some(s.to_string());
                            self.append_log(&format!("Video selected: {}", s));
                        }
                    }
                }
                if let Some(v) = &self.video_path {
                    ui.label(v);
                }
            });
            ui.add_space(6.0);

            // — Select Audio File —
            ui.horizontal(|ui| {
                if ui.button("Select Audio File").clicked() {
                    if let Some(p) = FileDialog::new()
                        .add_filter(
                            "Audio / Dubbed Video",
                            &["mp3", "wav", "aac", "mp4", "mkv", "avi", "flac"],
                        )
                        .set_title("Pick audio or dubbed‐video file")
                        .pick_file()
                    {
                        if let Some(s) = p.to_str() {
                            self.audio_path = Some(s.to_string());
                            self.append_log(&format!("Audio selected: {}", s));
                            self.probe_audio_tracks(s);
                        }
                    }
                }
                if let Some(a) = &self.audio_path {
                    ui.label(a);
                }
            });
            ui.add_space(6.0);

            // — If no “por” auto‐selected, show dropdown of all audio streams —
            if self.selected_audio_track.is_none() && !self.audio_tracks.is_empty() {
                ui.horizontal(|ui| {
                    ui.label("Choose audio stream:");
                    egui::ComboBox::from_id_source("audio_stream_combo")
                        .selected_text(if let Some(idx) = self.selected_audio_track {
                            format!("Track {}", idx)
                        } else {
                            "None".into()
                        })
                        .show_ui(ui, |ui| {
                            for (abs_idx, lang) in &self.audio_tracks {
                                let label = format!("Index {} ({})", abs_idx, lang);
                                if ui
                                    .selectable_label(
                                        Some(*abs_idx) == self.selected_audio_track,
                                        label,
                                    )
                                    .clicked()
                                {
                                    self.selected_audio_track = Some(*abs_idx);
                                }
                            }
                        });
                });
                ui.add_space(6.0);
            }

            // — Select Output File —
            ui.horizontal(|ui| {
                if ui.button("Select Output File").clicked() {
                    if let Some(p) = FileDialog::new()
                        .add_filter("Matroska MKV", &["mkv"])
                        .set_title("Save output as .mkv")
                        .save_file()
                    {
                        if let Some(s) = p.to_str() {
                            self.output_path = Some(s.to_string());
                            self.append_log(&format!("Output selected: {}", s));
                        }
                    }
                }
                if let Some(o) = &self.output_path {
                    ui.label(o);
                }
            });
            ui.add_space(8.0);

            // — Start Merge Button —
            if ui
                .add_enabled(!self.is_merging, egui::Button::new("Start Merge"))
                .clicked()
            {
                self.start_merge();
            }

            // — Spinner while merging —
            if self.is_merging {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Merging in progress…");
                });
            }

            ui.separator();

            // — Toggle for logs —
            if ui
                .button(if self.show_logs {
                    "Hide Logs"
                } else {
                    "Show Logs"
                })
                .clicked()
            {
                self.show_logs = !self.show_logs;
            }

            if self.show_logs {
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(250.0)
                    .show(ui, |ui| {
                        for line in &self.logs {
                            ui.label(line);
                        }
                    });
            }
        });

        // If merging, keep repainting for spinner animation
        if self.is_merging {
            ctx.request_repaint();
        }
    }
}

/// Run ffprobe on `file_path` → Vec<(stream_index, language_tag)>.
fn get_all_audio_tracks<F>(file_path: &str, log_fn: &F) -> Vec<AudioStream>
where
    F: Fn(&str),
{
    log_fn(&format!("ffprobe → probing: {}", file_path));
    let output = match Command::new("ffprobe")
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .args(&[
            "-hide_banner",
            "-loglevel",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=index:stream_tags=language",
            "-of",
            "json",
            file_path,
        ])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            log_fn(&format!("ffprobe failed to execute: {}", e));
            return Vec::new();
        }
    };
    if !output.status.success() {
        log_fn(&format!("ffprobe returned nonzero for {}", file_path));
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    log_fn(&format!("ffprobe JSON: {}", stdout));
    let parsed: Value = match serde_json::from_str(&stdout) {
        Ok(v) => v,
        Err(e) => {
            log_fn(&format!("JSON parse error: {}", e));
            return Vec::new();
        }
    };
    let streams = match parsed.get("streams").and_then(|s| s.as_array()) {
        Some(arr) => arr,
        None => {
            log_fn("No “streams” array in ffprobe output");
            return Vec::new();
        }
    };

    let mut result = Vec::new();
    for s in streams {
        if let Some(idx) = s.get("index").and_then(|v| v.as_u64()) {
            let lang = s
                .get("tags")
                .and_then(|t| t.get("language"))
                .and_then(|l| l.as_str())
                .unwrap_or("unknown")
                .to_string();
            log_fn(&format!("Found stream {} with lang \"{}\"", idx, lang));
            result.push((idx as u32, lang));
        }
    }
    result
}

/// Return the first audio stream whose `language` tag (ci‐insensitive) matches `language_code`.
fn find_audio_track<F>(file_path: &str, language_code: &str, log_fn: &F) -> Option<u32>
where
    F: Fn(&str),
{
    let tracks = get_all_audio_tracks(file_path, log_fn);
    for (idx, lang) in tracks {
        if lang.eq_ignore_ascii_case(language_code) {
            return Some(idx);
        }
    }
    None
}

fn main() {
    // 1) Possibly re‐launch elevated exactly once
    ensure_admin();

    // 2) Initialize simplelog (no console window appears due to `[windows_subsystem]`)
    let log_file = File::create("audio_merger.log").expect("Cannot create log file");
    let config = ConfigBuilder::new().build();
    CombinedLogger::init(vec![WriteLogger::new(
        log::LevelFilter::Info,
        config,
        log_file,
    )])
    .expect("Failed to initialize logger");
    info!("Application started.");

    // 3) Launch GUI in a maximized window
    let mut opts = NativeOptions::default();
    opts.maximized = true; // open window maximized
    run_native(
        "by TeusDev",
        opts,
        Box::new(|_cc| Box::new(AudioMergerApp::default())),
    );
}
