use rfd::FileDialog;
use serde_json::Value;
use std::process::Command;

/// Runs ffprobe on the given file and returns the audio stream index
/// for the first stream whose language tag matches `language_code` (case-insensitive).
fn find_audio_track(file_path: &str, language_code: &str) -> Option<u32> {
    println!("Running ffprobe on file: {}", file_path);
    let output = Command::new("ffprobe")
        .args(&[
            "-v",
            "error",
            "-select_streams",
            "a", // audio streams only
            "-show_entries",
            "stream=index:stream_tags=language",
            "-of",
            "json",
            file_path,
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        eprintln!("ffprobe failed for file: {}", file_path);
        return None;
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    println!("ffprobe output:\n{}", stdout_str);

    let parsed: Value = serde_json::from_str(&stdout_str).ok()?;
    println!("Parsed JSON: {}", parsed);
    let streams = parsed.get("streams")?.as_array()?;
    println!("Found {} audio stream(s).", streams.len());

    for stream in streams {
        let index = stream.get("index")?.as_u64().unwrap_or_default();
        let language = stream
            .get("tags")
            .and_then(|tags| tags.get("language"))
            .and_then(|lang| lang.as_str())
            .unwrap_or("unknown");
        println!("Audio stream index: {} with language: {}", index, language);
        if language.eq_ignore_ascii_case(language_code) {
            println!("Matching stream found: index {}", index);
            return stream.get("index")?.as_u64().map(|idx| idx as u32);
        }
    }
    None
}

fn main() {
    // This program prints outputs to the console.
    println!("Starting application.");

    // Ask the user to select the video file.
    let video_file = FileDialog::new()
        .add_filter("Video Files", &["mp4", "mkv", "avi"])
        .set_title("Select Video File")
        .pick_file();

    let video_file = match video_file {
        Some(path) => path,
        None => {
            println!("No video file selected.");
            return;
        }
    };

    let video_file_str = video_file.to_str().unwrap();
    println!("Video file selected: {}", video_file_str);

    // Find the English audio track in the video file.
    // If no English track is found, default to the original main audio (assumed at index 0).
    let eng_track_index = match find_audio_track(video_file_str, "eng") {
        Some(idx) => idx.saturating_sub(1),
        None => {
            println!(
                "No English audio track found in the video file. Using original main audio track."
            );
            0
        }
    };
    println!(
        "Using English audio track index (after subtracting 1 if applicable): {}",
        eng_track_index
    );

    // Ask the user to select the audio file (or a video file with dubbed audio).
    let audio_file = FileDialog::new()
        .add_filter(
            "Audio/Video Files",
            &["mp3", "wav", "aac", "mp4", "mkv", "avi"],
        )
        .set_title("Select Audio File (or Video File with Dubbed Audio)")
        .pick_file();

    let audio_file = match audio_file {
        Some(path) => path,
        None => {
            println!("No audio file selected.");
            return;
        }
    };

    let audio_file_str = audio_file.to_str().unwrap();
    println!("Audio file selected: {}", audio_file_str);

    // Find the Portuguese (Brazilian) audio track in the audio file.
    let pt_br_track_index = match find_audio_track(audio_file_str, "por") {
        Some(idx) => idx.saturating_sub(1),
        None => {
            println!("No Portuguese (Brazilian) audio track found in the audio file.");
            return;
        }
    };
    println!(
        "Using PT-BR audio track index (after subtracting 1): {}",
        pt_br_track_index
    );

    // Ask the user for the output file location.
    let output_file = FileDialog::new()
        .set_title("Save Output File")
        .add_filter("Matroska Video Files", &["mkv"])
        .save_file();

    let output_file = match output_file {
        Some(path) => path,
        None => {
            println!("No output file location selected.");
            return;
        }
    };

    let output_file_str = output_file.to_str().unwrap();
    println!("Output file: {}", output_file_str);

    // Use ffmpeg to merge video from the video file with the selected audio tracks.
    let status = Command::new("ffmpeg")
        .args(&[
            "-i",
            video_file_str,
            "-i",
            audio_file_str,
            "-map",
            "0:v:0",
            "-map",
            &format!("0:a:{}", eng_track_index),
            "-map",
            &format!("1:a:{}", pt_br_track_index),
            "-c",
            "copy",
            output_file_str,
        ])
        .status();

    match status {
        Ok(status) if status.success() => {
            println!("Output file created successfully!");
        }
        Ok(status) => {
            eprintln!("ffmpeg exited with status code: {}", status);
        }
        Err(e) => {
            eprintln!("Failed to run ffmpeg: {}", e);
        }
    }
}
