use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, bail, Context};
use clap::Parser;

// All famitracker exported .wav files have this much seconds of dead air at the start and end of the track
const FAMITRACKER_SILENCE_START: f64 = 0.084;
const FAMITRACKER_SILENCE_END: f64 = 0.1;

#[derive(Parser)]
#[command(about = "Mix Famitracker mono WAV stems into a stereo OGG")]
struct Args {
    /// Folder containing the exported .wav stems (defaults to the current directory)
    folder: Option<PathBuf>,
}

#[derive(Clone, Copy)]
struct Pan {
    left: f64,
    right: f64,
}

impl Pan {
    const HARD_LEFT: Self = Self {
        left: 1.0,
        right: 0.0,
    };
    const LEFT: Self = Self {
        left: 0.7,
        right: 0.3,
    };
    const RIGHT: Self = Self {
        left: 0.3,
        right: 0.7,
    };
    const HARD_RIGHT: Self = Self {
        left: 0.0,
        right: 1.0,
    };
    const CENTER: Self = Self {
        left: 1.0,
        right: 1.0,
    };

    fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_uppercase().as_str() {
            "" => Some(Self::CENTER),
            "1" | "HL" => Some(Self::HARD_LEFT),
            "2" | "L" => Some(Self::LEFT),
            "3" | "R" => Some(Self::RIGHT),
            "4" | "HR" => Some(Self::HARD_RIGHT),
            _ => None,
        }
    }

    fn filter(self) -> String {
        format!("pan=stereo|c0={}*c0|c1={}*c0", self.left, self.right)
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let folder = match args.folder {
        Some(folder) => folder,
        None => std::env::current_dir().context("failed to get current directory")?,
    };
    run(&folder)
}

fn run(folder: &Path) -> anyhow::Result<()> {
    if !folder.is_dir() {
        bail!("folder does not exist or is not a directory: {}", folder.display());
    }

    let wavs = collect_wavs(folder)?;
    if wavs.is_empty() {
        bail!("no .wav files found in {}", folder.display());
    }

    let mut pans = Vec::with_capacity(wavs.len());
    for wav in &wavs {
        let name = wav
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        pans.push(prompt_pan(name)?);
    }

    let output_name = prompt_output_name()?;
    let output_path = folder.join(format!("{output_name}.ogg"));

    run_ffmpeg(&wavs, &pans, &output_path)?;
    println!("Wrote {}", output_path.display());
    if let Err(err) = open_in_explorer(&output_path) {
        eprintln!("Could not open File Explorer: {err}");
    }
    Ok(())
}

fn open_in_explorer(path: &Path) -> anyhow::Result<()> {
    let path = explorer_path(path);
    Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .context("failed to launch File Explorer")?;
    Ok(())
}

fn explorer_path(path: &Path) -> PathBuf {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    match canonical.to_str() {
        Some(s) => PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(s)),
        None => canonical,
    }
}

fn collect_wavs(folder: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut wavs = Vec::new();
    for entry in folder
        .read_dir()
        .with_context(|| format!("failed to read {}", folder.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_wav = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"));
        if is_wav {
            wavs.push(path);
        }
    }
    wavs.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(wavs)
}

fn prompt_pan(track: &str) -> anyhow::Result<Pan> {
    loop {
        println!("{track}");
        println!("  [1] Hard Left   [2] Left   [3] Right   [4] Hard Right");
        println!("  Press Enter for no panning");
        print!("> ");
        io::stdout().flush()?;

        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .context("failed to read panning choice")?;

        if let Some(pan) = Pan::parse(&line) {
            return Ok(pan);
        }
        println!("Invalid choice. Enter 1-4, HL, L, R, HR, or leave blank.\n");
    }
}

fn prompt_output_name() -> anyhow::Result<String> {
    loop {
        print!("Output name (no extension): ");
        io::stdout().flush()?;

        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .context("failed to read output name")?;

        let mut name = line.trim().to_string();
        if name.is_empty() {
            println!("Name cannot be empty.");
            continue;
        }

        if let Some(stripped) = name.strip_suffix(".ogg").or_else(|| name.strip_suffix(".OGG")) {
            name = stripped.to_string();
        }

        if name.is_empty()
            || name.contains(['/', '\\'])
            || Path::new(&name)
                .file_name()
                .and_then(|n| n.to_str())
                != Some(name.as_str())
        {
            println!("Name cannot be empty or contain path separators.");
            continue;
        }

        return Ok(name);
    }
}

fn run_ffmpeg(wavs: &[PathBuf], pans: &[Pan], output_path: &Path) -> anyhow::Result<()> {
    let filter = build_filter_complex(wavs.len(), pans);

    let mut command = Command::new("ffmpeg");
    command.arg("-y");
    for wav in wavs {
        command.arg("-i").arg(wav);
    }
    command
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-map")
        .arg("[out]")
        .arg("-c:a")
        .arg("libvorbis")
        .arg(output_path);

    let output = command
        .output()
        .context("failed to run ffmpeg (is it installed and on PATH?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "ffmpeg failed with status {}:\n{stderr}",
            output.status
        ));
    }

    Ok(())
}

fn build_filter_complex(input_count: usize, pans: &[Pan]) -> String {
    let mut filter = String::new();

    for (i, pan) in pans.iter().enumerate() {
        filter.push_str(&format!("[{i}:a]{}[a{i}];", pan.filter()));
    }

    for i in 0..input_count {
        filter.push_str(&format!("[a{i}]"));
    }

    filter.push_str(&format!(
        "amix=inputs={input_count}:duration=longest:dropout_transition=0:normalize=0,\
         atrim=start={FAMITRACKER_SILENCE_START},asetpts=PTS-STARTPTS,\
         areverse,atrim=start={FAMITRACKER_SILENCE_END},asetpts=PTS-STARTPTS,\
         areverse,volume=3dB[out]"
    ));

    filter
}
