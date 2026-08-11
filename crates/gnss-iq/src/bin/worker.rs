//! `gnss-iq-worker`: run one job spec to completion.
//!
//! Invoked as a child process by the job queue, one process per job. That
//! isolation is the point: IQ synthesis allocates large buffers and runs for
//! minutes, and a job that fails -- on malformed RINEX, on a full disk --
//! takes its own process down and nothing else.
//!
//! Progress and the final result are reported as one JSON object per line on
//! stdout, so the parent can stream status without a side channel. Diagnostics
//! go to stderr and are never interleaved into that stream.
//!
//! ```text
//! gnss-iq-worker --job spec.json --id <job-id> --out <dir> [--nav <file>]...
//!                [--cache <dir>]
//! ```
//!
//! With `--nav`, exactly those files are used. With `--cache`, the BKG
//! filenames for the window are resolved inside that directory. `--nav` wins
//! if both are given.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use gnss_iq::ephemeris::{DiskCacheSource, EphemerisSource, LocalFileSource};
use gnss_iq::pipeline::{run_job, Progress};
use gnss_iq::JobSpec;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            // The parent parses stdout; a failure has to arrive there too, or
            // a crashed job looks identical to a job that produced nothing.
            emit(&serde_json::json!({ "event": "failed", "error": message }));
            eprintln!("gnss-iq-worker: {message}");
            ExitCode::FAILURE
        }
    }
}

struct Arguments {
    job: PathBuf,
    id: String,
    out: PathBuf,
    nav: Vec<PathBuf>,
    cache: Option<PathBuf>,
}

fn run() -> Result<(), String> {
    let arguments = parse_arguments()?;

    let text = std::fs::read_to_string(&arguments.job)
        .map_err(|e| format!("could not read job spec {}: {e}", arguments.job.display()))?;
    let spec: JobSpec =
        serde_json::from_str(&text).map_err(|e| format!("job spec is not valid: {e}"))?;

    let source: Box<dyn EphemerisSource> = if arguments.nav.is_empty() {
        let cache = arguments
            .cache
            .ok_or_else(|| "one of --nav or --cache is required".to_string())?;
        Box::new(DiskCacheSource::new(cache))
    } else {
        Box::new(LocalFileSource::new(arguments.nav))
    };

    emit(&serde_json::json!({ "event": "started", "job_id": arguments.id }));

    // Rate-limit progress to whole percents. At a million samples a second the
    // synthesis callback fires far more often than a parent needs to hear.
    let mut last_percent = u8::MAX;
    let artifacts = run_job(
        &arguments.id,
        &spec,
        source.as_ref(),
        &arguments.out,
        &mut |progress| {
            let percent = match progress {
                Progress::Synthesis { fraction } => (fraction * 100.0) as u8,
                _ => u8::MAX,
            };
            if percent != last_percent || percent == u8::MAX {
                last_percent = percent;
                emit(&serde_json::json!({ "event": "progress", "progress": progress }));
            }
        },
    )
    .map_err(|e| e.to_string())?;

    emit(&serde_json::json!({
        "event": "complete",
        "job_id": arguments.id,
        "binary_path": artifacts.binary_path,
        "sidecar_path": artifacts.sidecar_path,
        "samples": artifacts.summary.samples_written,
        "satellites": artifacts.sidecar.synthesis.satellites_generated,
        "clipped_samples": artifacts.summary.clipped_samples,
    }));

    Ok(())
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut job = None;
    let mut id = None;
    let mut out = None;
    let mut nav = Vec::new();
    let mut cache = None;

    let mut arguments = std::env::args().skip(1);
    while let Some(flag) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--job" => job = Some(PathBuf::from(value()?)),
            "--id" => id = Some(value()?),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--nav" => nav.push(PathBuf::from(value()?)),
            "--cache" => cache = Some(PathBuf::from(value()?)),
            "--help" | "-h" => {
                println!(
                    "usage: gnss-iq-worker --job <spec.json> --id <job-id> --out <dir> \
                     [--nav <file>]... [--cache <dir>]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }

    Ok(Arguments {
        job: job.ok_or("--job is required")?,
        id: id.ok_or("--id is required")?,
        out: out.ok_or("--out is required")?,
        nav,
        cache,
    })
}

/// One JSON object per line, flushed immediately.
///
/// Flushing matters: the parent reads this stream to decide a job is alive,
/// and a buffered pipe would make a long synthesis look like a hang.
fn emit(value: &serde_json::Value) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{value}");
    let _ = stdout.flush();
}
