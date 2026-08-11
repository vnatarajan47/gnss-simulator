//! End-to-end job execution: spec in, files out.
//!
//! The stages are deliberately separable -- [`crate::observables`] can be run
//! without generating a single sample, which is what lets the acquisition
//! cross-check compare a recovered measurement against the truth it was
//! generated from. This module is only the wiring.

use std::path::{Path, PathBuf};

use gnss_core::{BroadcastEphemeris, EphemerisSet, KeplerianEphemeris, SelectionConfig, Sv};

use crate::ephemeris::EphemerisSource;
use crate::job::JobSpec;
use crate::noise::build_models;
use crate::observables::{self, Observables};
use crate::sink::{output_names, Sidecar};
use crate::synth::{self, SynthesisSummary};
use crate::{Error, ValidatedJob};

/// Which stage a job is in, and how far through it is.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum Progress {
    /// Loading and parsing broadcast ephemeris.
    Ephemeris,
    /// Computing geometry and measurements.
    Observables,
    /// Writing samples. `fraction` runs 0 to 1.
    Synthesis { fraction: f64 },
    /// Writing the sidecar.
    Finalising,
}

/// What a completed job produced.
#[derive(Debug, Clone)]
pub struct JobArtifacts {
    pub binary_path: PathBuf,
    pub sidecar_path: PathBuf,
    pub sidecar: Sidecar,
    pub summary: SynthesisSummary,
    pub observables: Observables,
}

/// Run a job to completion, writing both output files into `output_dir`.
///
/// `progress` is called often enough to drive a status display and rarely
/// enough not to dominate the run; the synthesis stage reports about once per
/// percent.
pub fn run_job(
    job_id: &str,
    spec: &JobSpec,
    source: &dyn EphemerisSource,
    output_dir: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<JobArtifacts, Error> {
    let job = spec.validate()?;

    progress(Progress::Ephemeris);
    let loaded = source.load(spec.window.start_unix_s, spec.window.duration_s)?;

    progress(Progress::Observables);
    let models = build_models(&spec.noise);
    let observables = observables::compute(&job, &loaded.set, &models)?;

    std::fs::create_dir_all(output_dir).map_err(|e| Error::io(output_dir.display(), e))?;
    let names = output_names(job_id);
    let binary_path = output_dir.join(&names["binary"]);
    let sidecar_path = output_dir.join(&names["sidecar"]);

    let broadcast = BroadcastSelection::new(&loaded.set, &job);

    let mut file =
        std::fs::File::create(&binary_path).map_err(|e| Error::io(binary_path.display(), e))?;

    let summary = synth::synthesize(
        &job,
        &observables,
        &|sv| broadcast.for_satellite(sv),
        &mut file,
        &mut |fraction| progress(Progress::Synthesis { fraction }),
    )?;

    progress(Progress::Finalising);
    let sidecar = Sidecar::build(
        job_id,
        &job,
        &observables,
        &models,
        &summary,
        loaded.sources.clone(),
        now_unix_seconds(),
    );
    sidecar.write(&sidecar_path)?;

    Ok(JobArtifacts {
        binary_path,
        sidecar_path,
        sidecar,
        summary,
        observables,
    })
}

/// Picks the ephemeris block each satellite will *broadcast* in its LNAV
/// frames.
///
/// Deliberately one block for the whole window, chosen nearest the window
/// midpoint, rather than the per-epoch selection the geometry uses. A real
/// satellite transmits one ephemeris for roughly two hours and then switches,
/// so a job shorter than that genuinely sees a single set. Switching mid-file
/// would be more faithful for long windows, but it would also mean the
/// broadcast parameters stop matching the geometry at the switch instant
/// unless the changeover is modelled exactly -- and an inconsistency there
/// shows up as a position error in any receiver that decodes the file.
struct BroadcastSelection<'a> {
    set: &'a EphemerisSet,
    midpoint: gnss_core::GpsTime,
}

impl<'a> BroadcastSelection<'a> {
    fn new(set: &'a EphemerisSet, job: &ValidatedJob) -> Self {
        Self {
            set,
            midpoint: job.start.offset_by(job.spec.window.duration_s / 2.0),
        }
    }

    fn for_satellite(&self, sv: Sv) -> Option<KeplerianEphemeris> {
        match self.set.select(sv, self.midpoint, SelectionConfig::default())? {
            BroadcastEphemeris::Keplerian(ephemeris) => Some(*ephemeris),
            BroadcastEphemeris::Sbas(_) => None,
        }
    }
}

fn now_unix_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ephemeris::LocalFileSource;
    use crate::job::{
        Cn0Spec, ConstellationSpec, NoiseSpec, OutputSpec, PseudorangeNoiseSpec, Quantization,
        ReceiverSpec, WindowSpec,
    };
    use crate::signal::Band;

    fn fixture_source() -> LocalFileSource {
        LocalFileSource::new([PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/BRDC00WRD_R_20250010000_01D_GN.rnx")])
    }

    fn spec() -> JobSpec {
        JobSpec {
            constellation: ConstellationSpec::Gps,
            band: Band::L1,
            receiver: ReceiverSpec::Static {
                latitude_deg: 39.7392,
                longitude_deg: -104.9903,
                altitude_m: 1609.0,
            },
            window: WindowSpec {
                start_unix_s: 1_735_732_800.0,
                duration_s: 0.05,
                epoch_interval_s: 0.01,
            },
            output: OutputSpec {
                sample_rate_hz: 2.048e6,
                quantization: Quantization::Int8,
                center_frequency_hz: 1_575.42e6,
            },
            elevation_mask_deg: 5.0,
            noise: NoiseSpec {
                pseudorange: PseudorangeNoiseSpec::Fixed { sigma_m: 1.0 },
                cn0: Cn0Spec::Elevation,
            },
            seed: Some(2025),
        }
    }

    #[test]
    fn a_job_runs_end_to_end_and_writes_both_files() {
        let output = std::env::temp_dir().join(format!("gnss-iq-test-{}", std::process::id()));
        let artifacts = run_job(
            "test-job",
            &spec(),
            &fixture_source(),
            &output,
            &mut |_| {},
        )
        .unwrap();

        assert!(artifacts.binary_path.exists());
        assert!(artifacts.sidecar_path.exists());

        // 0.05 s at 2.048 MHz, int8 interleaved: two bytes per sample.
        let expected_samples = 102_400u64;
        assert_eq!(artifacts.summary.samples_written, expected_samples);
        let size = std::fs::metadata(&artifacts.binary_path).unwrap().len();
        assert_eq!(size, expected_samples * 2);

        // The recording should not be clipping at eight sigma of headroom.
        assert_eq!(artifacts.summary.clipped_samples, 0);

        let sidecar: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&artifacts.sidecar_path).unwrap())
                .unwrap();

        assert_eq!(sidecar["job_id"], "test-job");
        assert_eq!(sidecar["signal"]["id"], "GPS_L1CA");
        assert_eq!(sidecar["format"]["byte_order"], "little");
        assert_eq!(sidecar["format"]["interleaving"], "IQIQ");
        assert_eq!(sidecar["format"]["quantization"], "int8");
        assert_eq!(sidecar["format"]["sample_rate_hz"], 2.048e6);

        // The brief's exact requirement: which models ran, with their
        // parameters.
        assert_eq!(sidecar["noise_models"]["pseudorange"]["model"], "fixed");
        assert_eq!(sidecar["noise_models"]["pseudorange"]["sigma_m"], 1.0);
        assert_eq!(sidecar["noise_models"]["cn0"]["model"], "elevation");
        assert_eq!(sidecar["noise_models"]["cn0"]["cn0_max"], 45.0);
        assert_eq!(sidecar["noise_models"]["cn0"]["attenuation"], 12.0);
        assert_eq!(sidecar["noise_models"]["cn0"]["decay_deg"], 12.0);
        assert_eq!(sidecar["noise_models"]["seed"], 2025);

        // Provenance of the substituted ionosphere.
        assert_eq!(sidecar["atmosphere"]["ionosphere_source"], "fallback");
        assert!(sidecar["satellites"].as_array().unwrap().len() >= 6);

        std::fs::remove_dir_all(&output).ok();
    }

    /// The same seed must reproduce the file byte for byte. This is what the
    /// sidecar's seed field promises, and it also pins that nothing in the
    /// pipeline depends on iteration order of a hash map.
    #[test]
    fn the_same_seed_reproduces_the_file_byte_for_byte() {
        let root = std::env::temp_dir().join(format!("gnss-iq-repeat-{}", std::process::id()));

        let first = run_job("a", &spec(), &fixture_source(), &root.join("a"), &mut |_| {}).unwrap();
        let second = run_job("b", &spec(), &fixture_source(), &root.join("b"), &mut |_| {}).unwrap();

        let left = std::fs::read(&first.binary_path).unwrap();
        let right = std::fs::read(&second.binary_path).unwrap();
        assert_eq!(left, right, "identical jobs produced different samples");

        // And a different seed must not.
        let mut different = spec();
        different.seed = Some(99);
        let third =
            run_job("c", &different, &fixture_source(), &root.join("c"), &mut |_| {}).unwrap();
        assert_ne!(left, std::fs::read(&third.binary_path).unwrap());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn progress_reaches_every_stage_and_ends_at_one() {
        let output = std::env::temp_dir().join(format!("gnss-iq-progress-{}", std::process::id()));
        let mut seen = Vec::new();
        run_job("p", &spec(), &fixture_source(), &output, &mut |p| {
            seen.push(p)
        })
        .unwrap();

        assert!(seen.contains(&Progress::Ephemeris));
        assert!(seen.contains(&Progress::Observables));
        assert!(seen.contains(&Progress::Finalising));
        assert!(seen
            .iter()
            .any(|p| matches!(p, Progress::Synthesis { fraction } if *fraction >= 1.0)));

        std::fs::remove_dir_all(&output).ok();
    }

    /// Float output is written unscaled, so it is the reference the integer
    /// formats can be checked against -- and it must be exactly four times the
    /// size of the int8 file.
    #[test]
    fn float_output_is_unscaled_and_wider() {
        let output = std::env::temp_dir().join(format!("gnss-iq-float-{}", std::process::id()));
        let mut float_spec = spec();
        float_spec.output.quantization = Quantization::Float32;

        let artifacts =
            run_job("f", &float_spec, &fixture_source(), &output, &mut |_| {}).unwrap();
        assert_eq!(artifacts.summary.scale_factor, 1.0);
        assert_eq!(
            std::fs::metadata(&artifacts.binary_path).unwrap().len(),
            artifacts.summary.samples_written * 8
        );

        std::fs::remove_dir_all(&output).ok();
    }
}
