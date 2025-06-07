use std::path::PathBuf;
use std::process;
use std::{env, fs, io};

// Build dependencies: reqwest = { version = "0.11", features = ["blocking"] }, zip = "0.6", walkdir = "2"
// This build script downloads a prebuilt FFmpeg zip and bundles ffmpeg.exe and ffprobe.exe alongside the release binary.
fn main() {
    // Only run bundler in release on Windows
    let profile = env::var("PROFILE").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if profile != "release" || target_os != "windows" {
        return;
    }

    // Determine release directory (target/release)
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let release_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("Failed to locate release directory")
        .to_path_buf();

    // Cache directory for download and extraction
    let cache_dir = out_dir.join("ffmpeg-cache");
    fs::create_dir_all(&cache_dir).expect("Failed to create cache directory");

    // Download URL (BtbN latest Windows builds)
    let url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";
    let archive_path = cache_dir.join("ffmpeg.zip");

    // Download if not already cached
    if !archive_path.exists() {
        println!("Downloading FFmpeg from {}…", url);
        let mut resp = reqwest::blocking::get(url).expect("Failed to GET FFmpeg archive");
        assert!(
            resp.status().is_success(),
            "Download failed: {}",
            resp.status()
        );
        let mut out = fs::File::create(&archive_path).expect("Failed to create archive file");
        io::copy(&mut resp, &mut out).expect("Failed to write FFmpeg archive");
    }

    // Extraction directory
    let extract_dir = cache_dir.join("extracted");
    if !extract_dir.exists() {
        fs::create_dir_all(&extract_dir).expect("Failed to create extract directory");
        println!("Extracting FFmpeg archive…");
        let file = fs::File::open(&archive_path).expect("Cannot open FFmpeg archive");
        let mut archive = zip::ZipArchive::new(file).expect("Failed to read zip archive");
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            let outpath = match entry.enclosed_name() {
                Some(path) => extract_dir.join(path),
                None => continue,
            };
            if (&*entry.name()).ends_with('/') {
                fs::create_dir_all(&outpath).unwrap();
            } else {
                if let Some(parent) = outpath.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                let mut outfile = fs::File::create(&outpath).unwrap();
                io::copy(&mut entry, &mut outfile).unwrap();
            }
        }
    }

    // Locate ffmpeg.exe and ffprobe.exe under extracted contents
    let mut bin_dir: Option<PathBuf> = None;
    for entry in walkdir::WalkDir::new(&extract_dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case("ffmpeg.exe")
        {
            bin_dir = entry.path().parent().map(|p| p.to_path_buf());
            break;
        }
    }
    let bin_dir = bin_dir.expect("Could not find ffmpeg.exe in extracted archive");

    // Copy executables to release directory
    for exe in &["ffmpeg.exe", "ffprobe.exe"] {
        let src = bin_dir.join(exe);
        let dst = release_dir.join(exe);
        println!("cargo:rerun-if-changed={}", src.display());
        fs::copy(&src, &dst).unwrap_or_else(|e| {
            panic!(
                "Failed to copy {} to {}: {}",
                src.display(),
                dst.display(),
                e
            )
        });
    }

    // Optionally strip symbols if strip is available
    for exe in &["ffmpeg.exe", "ffprobe.exe"] {
        let p = release_dir.join(exe);
        let _ = process::Command::new("strip").arg(&p).status();
    }
}
