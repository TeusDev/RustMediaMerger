#![windows_subsystem = "windows"]
//! Audio Merger GUI: bundle ffmpeg/ffprobe alongside exe, maximized, toggleable logs.

use eframe::{egui, run_native, App, Frame, NativeOptions};
use log::info;
use rfd::FileDialog;
use serde_json::Value;
use simplelog::{CombinedLogger, ConfigBuilder, WriteLogger};
use std::fs::File;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// (stream_index, language_tag)
type AudioStream = (u32, String);

/// The main application state and UI logic.
struct AudioMergerApp {
    video_path: Option<String>,     // input video file
    audio_path: Option<String>,     // external audio or dubbed video
    output_path: Option<String>,    // output .mkv file
    audio_tracks: Vec<AudioStream>, // extracted from audio_path
    selected_track: Option<u32>,    // chosen audio stream index
    logs: Vec<String>,
    rx: Receiver<String>,
    tx: Sender<String>,
    is_merging: bool,
    show_logs: bool,
    exe_dir: PathBuf, // folder containing ffmpeg/ffprobe
}

impl Default for AudioMergerApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let exe = std::env::current_exe().unwrap();
        let dir = exe.parent().unwrap().to_path_buf();
        Self {
            video_path: None,
            audio_path: None,
            output_path: None,
            audio_tracks: Vec::new(),
            selected_track: None,
            logs: Vec::new(),
            rx,
            tx,
            is_merging: false,
            show_logs: false,
            exe_dir: dir,
        }
    }
}

impl AudioMergerApp {
    /// Append message to on-screen log and file logger
    fn append_log(&mut self, msg: &str) {
        self.logs.push(msg.to_string());
        info!("{}", msg);
    }

    /// Probe the external audio file for its streams, auto-select "por" if present.
    fn probe_audio_tracks(&mut self) {
        let path = match &self.audio_path {
            Some(p) => p.clone(),
            None => return,
        };
        self.append_log(&format!("Probing audio streams in: {}", path));
        self.audio_tracks = get_all_audio_tracks(&path, &self.exe_dir);
        self.selected_track = self.audio_tracks.iter().find_map(|(i, l)| {
            if l.eq_ignore_ascii_case("por") {
                Some(*i)
            } else {
                None
            }
        });
        if let Some(i) = self.selected_track {
            self.append_log(&format!("Auto-selected Portuguese track {}", i));
        } else if self.audio_tracks.is_empty() {
            self.append_log("No audio streams found in external file.");
        } else {
            self.append_log("Please select an audio stream from the dropdown.");
        }
    }

    /// Start the merge thread: finds "eng" in video, then merges with selected external track.
    fn start_merge(&mut self) {
        // validate inputs
        let video = match &self.video_path {
            Some(v) => v.clone(),
            None => {
                self.append_log("Error: select video file");
                return;
            }
        };
        let audio = match &self.audio_path {
            Some(a) => a.clone(),
            None => {
                self.append_log("Error: select audio file");
                return;
            }
        };
        let output = match &self.output_path {
            Some(o) => o.clone(),
            None => {
                self.append_log("Error: select output file");
                return;
            }
        };
        let track = match self.selected_track {
            Some(t) => t,
            None => {
                self.append_log("Error: pick audio stream");
                return;
            }
        };
        if self.is_merging {
            self.append_log("Merge already in progress");
            return;
        }
        self.is_merging = true;
        self.append_log(&format!(
            "Merging: video='{}', audio='{}', track={}, output='{}'",
            video, audio, track, output
        ));
        let tx = self.tx.clone();
        let exe_dir = self.exe_dir.clone();
        thread::spawn(move || {
            let logger = |m: &str| {
                let _ = tx.send(m.to_string());
            };
            // find English in video
            logger("ffprobe: searching for 'eng' in video...");
            let eng = find_audio_track(&video, "eng", &exe_dir);
            if let Some(idx) = eng {
                logger(&format!("Found video 'eng' stream {}", idx));
            } else {
                logger("No 'eng' in video, using track 0");
            }
            // build ffmpeg command
            let mut cmd = Command::new(exe_dir.join("ffmpeg.exe"));
            cmd.creation_flags(0x0800_0000).args(&[
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-i",
                &video,
                "-i",
                &audio,
            ]);
            // video + its audio
            if let Some(idx) = eng {
                cmd.args(&["-map", &format!("0:{}", idx)]);
            } else {
                cmd.args(&["-map", "0:0", "-map", "0:1"]);
            }
            // external audio
            cmd.args(&["-map", &format!("1:{}", track), "-c", "copy", &output]);
            logger("Running ffmpeg...");
            match cmd.status() {
                Ok(s) if s.success() => logger("Merge completed successfully"),
                Ok(s) => logger(&format!("ffmpeg exit code {:?}", s.code())),
                Err(e) => logger(&format!("Failed to run ffmpeg: {}", e)),
            }
            let _ = tx.send("MERGE_DONE".to_string());
        });
    }
}

impl App for AudioMergerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        // process background logs
        while let Ok(msg) = self.rx.try_recv() {
            if msg == "MERGE_DONE" {
                self.is_merging = false;
                self.append_log("Merge thread finished");
            } else {
                self.append_log(&msg);
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Audio Merger GUI");
            ui.separator();
            // video selector
            ui.horizontal(|ui| {
                if ui.button("Select Video File").clicked() {
                    if let Some(p) = FileDialog::new()
                        .add_filter("Video", &["mp4", "mkv", "avi"])
                        .pick_file()
                    {
                        if let Some(s) = p.to_str() {
                            self.video_path = Some(s.to_string());
                            self.append_log(&format!("Video: {}", s));
                        }
                    }
                }
                if let Some(v) = &self.video_path {
                    ui.label(v);
                }
            });
            ui.add_space(6.0);
            // audio selector
            ui.horizontal(|ui| {
                if ui.button("Select Audio/Dubbed File").clicked() {
                    if let Some(p) = FileDialog::new()
                        .add_filter("Audio/Video", &["mp3", "aac", "mp4", "mkv"])
                        .pick_file()
                    {
                        if let Some(s) = p.to_str() {
                            self.audio_path = Some(s.to_string());
                            self.append_log(&format!("Audio: {}", s));
                            self.probe_audio_tracks();
                        }
                    }
                }
                if let Some(a) = &self.audio_path {
                    ui.label(a);
                }
            });
            ui.add_space(6.0);
            // dropdown if needed
            if self.selected_track.is_none() && !self.audio_tracks.is_empty() {
                ui.horizontal(|ui| {
                    ui.label("Choose audio stream:");
                    egui::ComboBox::from_id_source("stream_combo")
                        .selected_text("None")
                        .show_ui(ui, |ui| {
                            for (i, lang) in &self.audio_tracks {
                                let txt = format!("Index {} ({})", i, lang);
                                if ui
                                    .selectable_label(Some(*i) == self.selected_track, txt)
                                    .clicked()
                                {
                                    self.selected_track = Some(*i);
                                }
                            }
                        });
                });
                ui.add_space(6.0);
            }
            // output selector
            ui.horizontal(|ui| {
                if ui.button("Select Output File").clicked() {
                    if let Some(p) = FileDialog::new()
                        .add_filter("Matroska MKV", &["mkv"])
                        .save_file()
                    {
                        if let Some(s) = p.to_str() {
                            self.output_path = Some(s.to_string());
                            self.append_log(&format!("Output: {}", s));
                        }
                    }
                }
                if let Some(o) = &self.output_path {
                    ui.label(o);
                }
            });
            ui.add_space(8.0);
            // merge button
            if ui
                .add_enabled(!self.is_merging, egui::Button::new("Start Merge"))
                .clicked()
            {
                self.start_merge();
            }
            // progress
            if self.is_merging {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Merging in progress...");
                });
            }
            ui.separator();
            // logs toggle
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
        if self.is_merging {
            ctx.request_repaint();
        }
    }
}

/// Bundled ffprobe extraction of audio streams
fn get_all_audio_tracks(file: &str, exe_dir: &PathBuf) -> Vec<AudioStream> {
    let ffprobe = exe_dir.join("ffprobe.exe");
    let out = Command::new(ffprobe)
        .creation_flags(0x0800_0000)
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
            file,
        ])
        .output();
    if let Ok(o) = out {
        if o.status.success() {
            if let Ok(txt) = String::from_utf8(o.stdout) {
                if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                    if let Some(arr) = v.get("streams").and_then(|s| s.as_array()) {
                        return arr
                            .iter()
                            .filter_map(|s| {
                                s.get("index").and_then(|i| i.as_u64()).map(|i| {
                                    let lang = s
                                        .get("tags")
                                        .and_then(|t| t.get("language"))
                                        .and_then(|l| l.as_str())
                                        .unwrap_or("unknown");
                                    (i as u32, lang.to_string())
                                })
                            })
                            .collect();
                    }
                }
            }
        }
    }
    Vec::new()
}

/// Find first stream matching language code
fn find_audio_track(file: &str, code: &str, exe_dir: &PathBuf) -> Option<u32> {
    get_all_audio_tracks(file, exe_dir)
        .into_iter()
        .find_map(|(i, l)| {
            if l.eq_ignore_ascii_case(code) {
                Some(i)
            } else {
                None
            }
        })
}

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use winapi::um::handleapi::CloseHandle;
use winapi::um::processthreadsapi::GetCurrentProcess;
use winapi::um::processthreadsapi::OpenProcessToken;
use winapi::um::securitybaseapi::GetTokenInformation;
use winapi::um::shellapi::ShellExecuteW;
use winapi::um::winnt::{TokenElevation, HANDLE, TOKEN_ELEVATION, TOKEN_QUERY};
use winapi::um::winuser::SW_SHOW;

/// Checks if current process has elevated privileges
fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;

        let result = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            size,
            &mut size,
        );

        CloseHandle(token);
        result != 0 && elevation.TokenIsElevated != 0
    }
}

/// Relaunches self with admin privileges via ShellExecuteW
fn relaunch_as_admin() {
    let exe = std::env::current_exe().unwrap();
    let exe_w: Vec<u16> = OsStr::new(exe.to_str().unwrap())
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        ShellExecuteW(
            null_mut(),
            widestring("runas").as_ptr(),
            exe_w.as_ptr(),
            null_mut(),
            null_mut(),
            SW_SHOW,
        );
    }
    std::process::exit(0);
}

fn widestring(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn main() {
    if !is_elevated() {
        relaunch_as_admin();
    }

    // logger to file
    let file = File::create("audio_merger.log").expect("Cannot create log");
    CombinedLogger::init(vec![WriteLogger::new(
        log::LevelFilter::Info,
        ConfigBuilder::new().build(),
        file,
    )])
    .unwrap();

    info!("Application started");
    let mut opts = NativeOptions::default();
    opts.viewport.maximized = Some(true);
    run_native(
        "Audio Merger GUI",
        opts,
        Box::new(|_cc| Box::new(AudioMergerApp::default())),
    )
    .unwrap();
}
