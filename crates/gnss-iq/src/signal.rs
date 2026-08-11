//! Which signal is being generated, and the parameters that follow from it.
//!
//! This is the module that keeps "GPS L1 C/A" from leaking into the rest of
//! the pipeline. Everything downstream -- observables, code generation,
//! synthesis, the sidecar -- asks a [`Signal`] for its carrier frequency, chip
//! rate, code length and data rate rather than referring to GPS constants
//! directly. Adding GPS L5 or Galileo E1 is then a new [`Signal`] variant plus
//! a spreading-code implementation, with no edit to the synthesis loop.
//!
//! The split between [`Band`] and [`Signal`] is deliberate. A band is an RF
//! centre frequency, shared across constellations (GPS L5 and Galileo E5a are
//! both 1176.45 MHz); a signal is a specific modulation on a band, and *that*
//! is what fixes the chip rate and code length. A job asks for a constellation
//! and a band, which resolve to at most one signal per constellation today.

use std::fmt;

use gnss_core::Constellation;

/// An RF band, identified by its centre frequency.
///
/// Every band a job can name is listed, including ones no [`Signal`] resolves
/// to yet: the schema is meant to accept the request and have resolution fail
/// with a specific message, rather than reject the field as unparseable and
/// force a schema change when support lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Band {
    L1,
    L2,
    L5,
    E1,
    E5a,
    E5b,
    B1I,
    B2a,
}

impl Band {
    /// Centre frequency \[Hz\].
    pub const fn centre_hz(self) -> f64 {
        match self {
            Band::L1 | Band::E1 | Band::B1I => 1_575.42e6,
            Band::L2 => 1_227.60e6,
            Band::L5 | Band::E5a | Band::B2a => 1_176.45e6,
            Band::E5b => 1_207.14e6,
        }
    }
}

impl fmt::Display for Band {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Band::L1 => "L1",
            Band::L2 => "L2",
            Band::L5 => "L5",
            Band::E1 => "E1",
            Band::E5a => "E5a",
            Band::E5b => "E5b",
            Band::B1I => "B1I",
            Band::B2a => "B2a",
        };
        f.write_str(name)
    }
}

/// A concrete spread-spectrum signal this crate can synthesise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    /// GPS L1 C/A: 1023-chip Gold code at 1.023 Mchip/s, 50 bps LNAV data.
    GpsL1Ca,
}

impl Signal {
    /// Resolve a constellation and band to a signal.
    ///
    /// `None` means the combination is real but not implemented; the caller
    /// reports which one was asked for. It does not distinguish "not yet" from
    /// "never" -- that judgement belongs in the error message at the boundary,
    /// not in the type.
    pub fn resolve(constellation: Constellation, band: Band) -> Option<Signal> {
        match (constellation, band) {
            (Constellation::Gps, Band::L1) => Some(Signal::GpsL1Ca),
            _ => None,
        }
    }

    pub const fn constellation(self) -> Constellation {
        match self {
            Signal::GpsL1Ca => Constellation::Gps,
        }
    }

    pub const fn band(self) -> Band {
        match self {
            Signal::GpsL1Ca => Band::L1,
        }
    }

    /// Nominal carrier frequency \[Hz\].
    pub const fn carrier_hz(self) -> f64 {
        self.band().centre_hz()
    }

    /// Spreading-code chip rate \[chip/s\].
    pub const fn chip_rate_hz(self) -> f64 {
        match self {
            Signal::GpsL1Ca => 1.023e6,
        }
    }

    /// Spreading-code length \[chips\].
    pub const fn code_length_chips(self) -> u32 {
        match self {
            Signal::GpsL1Ca => 1023,
        }
    }

    /// Navigation data rate \[bit/s\].
    pub const fn data_rate_hz(self) -> f64 {
        match self {
            Signal::GpsL1Ca => 50.0,
        }
    }

    /// Duration of one full spreading-code period \[s\].
    pub fn code_period_s(self) -> f64 {
        f64::from(self.code_length_chips()) / self.chip_rate_hz()
    }

    /// Whole code periods per navigation bit.
    ///
    /// 20 for GPS L1 C/A: a 1 ms code and a 20 ms bit. Integer by
    /// construction for every signal here, which is what lets the synthesiser
    /// index the data bit from the code-period counter.
    pub fn code_periods_per_bit(self) -> u32 {
        (1.0 / (self.data_rate_hz() * self.code_period_s())).round() as u32
    }

    /// The narrowest complex sample rate that keeps the main lobe of the
    /// spreading code \[Hz\].
    ///
    /// The null-to-null main lobe of a BPSK code is twice the chip rate wide.
    /// Sampling below this does not merely alias noise in: it removes signal
    /// power that a receiver's correlator expects, depressing the recovered
    /// C/N0 below the value the job asked for.
    pub fn minimum_sample_rate_hz(self) -> f64 {
        2.0 * self.chip_rate_hz()
    }

    /// Stable identifier for output metadata.
    pub const fn id(self) -> &'static str {
        match self {
            Signal::GpsL1Ca => "GPS_L1CA",
        }
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Signal::GpsL1Ca => f.write_str("GPS L1 C/A"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_l1_resolves_and_nothing_else_does_yet() {
        assert_eq!(
            Signal::resolve(Constellation::Gps, Band::L1),
            Some(Signal::GpsL1Ca)
        );
        // Real combinations that simply are not implemented. They must resolve
        // to None rather than panic or silently fall back to GPS L1 -- a
        // fallback here would hand back an L1 file labelled L5.
        assert_eq!(Signal::resolve(Constellation::Gps, Band::L5), None);
        assert_eq!(Signal::resolve(Constellation::Galileo, Band::E1), None);
        assert_eq!(Signal::resolve(Constellation::BeiDou, Band::B1I), None);
    }

    /// L1, E1 and B1I are the same physical carrier; L5, E5a and B2a likewise.
    /// Getting these apart would put the Doppler scaling wrong by 25%.
    #[test]
    fn bands_sharing_a_carrier_report_the_same_frequency() {
        assert_eq!(Band::L1.centre_hz(), Band::E1.centre_hz());
        assert_eq!(Band::L1.centre_hz(), Band::B1I.centre_hz());
        assert_eq!(Band::L5.centre_hz(), Band::E5a.centre_hz());
        assert_eq!(Band::L5.centre_hz(), Band::B2a.centre_hz());
        assert!(Band::L2.centre_hz() < Band::L1.centre_hz());
    }

    /// The C/A code is 1 ms long and a nav bit is 20 ms, so exactly 20 code
    /// periods per bit. The synthesiser indexes data bits from the code-period
    /// counter and would silently smear bit edges if this were not integral.
    #[test]
    fn gps_l1_ca_timing_relationships_hold() {
        let signal = Signal::GpsL1Ca;
        assert!((signal.code_period_s() - 1e-3).abs() < 1e-15);
        assert_eq!(signal.code_periods_per_bit(), 20);
        assert!((signal.minimum_sample_rate_hz() - 2.046e6).abs() < 1e-9);
        assert_eq!(signal.carrier_hz(), 1_575.42e6);
    }
}
