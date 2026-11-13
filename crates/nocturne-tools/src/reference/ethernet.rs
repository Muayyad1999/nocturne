//! Ethernet physical layers, with reach — the cabling cheat sheet.

use serde::{Deserialize, Serialize};

/// One Ethernet physical layer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CableEntry {
    /// The PHY name, e.g. `10GBASE-SR`.
    pub standard: String,
    /// What it runs over.
    pub medium: String,
    /// Nominal data rate, e.g. `10 Gb/s`.
    pub speed: String,
    /// Maximum reach.
    pub max_distance: String,
    /// Connector, lane count, caveats; empty when there is nothing to add.
    pub notes: String,
}

/// `(standard, medium, speed, max distance, notes)`, by speed. Reaches are
/// those of IEEE 802.3 unless the notes say the PHY is a vendor or MSA one.
#[rustfmt::skip]
const STANDARDS: &[(&str, &str, &str, &str, &str)] = &[
    ("10BASE5", "Thick coaxial cable", "10 Mb/s", "500 m", "Thicknet; obsolete"),
    ("10BASE2", "Thin coaxial cable", "10 Mb/s", "185 m", "Thinnet; obsolete"),
    ("10BASE-T", "Twisted pair, Cat 3 or better", "10 Mb/s", "100 m", "Two pairs"),
    ("10BASE-FL", "Multimode fibre, 850 nm", "10 Mb/s", "2 km", ""),
    ("100BASE-TX", "Twisted pair, Cat 5 or better", "100 Mb/s", "100 m", "Two pairs"),
    ("100BASE-FX", "Multimode fibre, 1310 nm", "100 Mb/s", "2 km", "Full duplex; 412 m in half duplex"),
    ("100BASE-LX10", "Single-mode fibre, 1310 nm", "100 Mb/s", "10 km", ""),
    ("100BASE-BX10", "Single-mode fibre, one strand, 1310/1550 nm", "100 Mb/s", "10 km", "Bidirectional; optics come in matched pairs"),
    ("1000BASE-T", "Twisted pair, Cat 5e or better", "1 Gb/s", "100 m", "All four pairs"),
    ("1000BASE-SX", "Multimode fibre, 850 nm", "1 Gb/s", "550 m (OM2 and better), 275 m (OM1)", "220 m on FDDI-grade 62.5 µm fibre"),
    ("1000BASE-LX", "Single-mode fibre, 1310 nm", "1 Gb/s", "5 km", "Also 550 m on multimode fibre with a mode-conditioning patch cord; vendor LX/LH optics reach 10 km"),
    ("1000BASE-LX10", "Single-mode fibre, 1310 nm", "1 Gb/s", "10 km", ""),
    ("1000BASE-BX10", "Single-mode fibre, one strand, 1310/1490 nm", "1 Gb/s", "10 km", "Bidirectional; optics come in matched pairs"),
    ("1000BASE-EX", "Single-mode fibre, 1310 nm", "1 Gb/s", "40 km", "Vendor optic, not an IEEE standard"),
    ("1000BASE-ZX", "Single-mode fibre, 1550 nm", "1 Gb/s", "70-80 km", "Vendor optic, not an IEEE standard; needs attenuators on short links"),
    ("2.5GBASE-T", "Twisted pair, Cat 5e or better", "2.5 Gb/s", "100 m", "IEEE 802.3bz (NBASE-T)"),
    ("5GBASE-T", "Twisted pair, Cat 6 or better", "5 Gb/s", "100 m", "IEEE 802.3bz (NBASE-T); usually works on good Cat 5e too"),
    ("10GBASE-T", "Twisted pair, Cat 6A or better", "10 Gb/s", "100 m", "55 m on Cat 6"),
    ("10GBASE-CR", "Twinax direct-attach cable (SFP+)", "10 Gb/s", "7 m passive, 15 m active", "SFP+ DAC per SFF-8431; not an IEEE PHY"),
    ("10GBASE-SR", "Multimode fibre, 850 nm", "10 Gb/s", "300 m (OM3), 400 m (OM4)", "33 m on OM1, 82 m on OM2"),
    ("10GBASE-LRM", "Multimode fibre, 1310 nm", "10 Gb/s", "220 m", "For legacy OM1 and OM2 plant"),
    ("10GBASE-LR", "Single-mode fibre, 1310 nm", "10 Gb/s", "10 km", ""),
    ("10GBASE-ER", "Single-mode fibre, 1550 nm", "10 Gb/s", "40 km", ""),
    ("10GBASE-ZR", "Single-mode fibre, 1550 nm", "10 Gb/s", "80 km", "Vendor optic, not an IEEE standard"),
    ("25GBASE-T", "Twisted pair, Cat 8", "25 Gb/s", "30 m", ""),
    ("25GBASE-CR", "Twinax direct-attach cable (SFP28)", "25 Gb/s", "5 m", "3 m for 25GBASE-CR-S, which runs without RS-FEC"),
    ("25GBASE-SR", "Multimode fibre, 850 nm", "25 Gb/s", "70 m (OM3), 100 m (OM4)", ""),
    ("25GBASE-LR", "Single-mode fibre, 1310 nm", "25 Gb/s", "10 km", ""),
    ("25GBASE-ER", "Single-mode fibre, 1310 nm", "25 Gb/s", "40 km", "30 km on ordinary links, 40 km on engineered ones"),
    ("40GBASE-T", "Twisted pair, Cat 8", "40 Gb/s", "30 m", ""),
    ("40GBASE-CR4", "Twinax direct-attach cable (QSFP+), 4 lanes", "40 Gb/s", "7 m", ""),
    ("40GBASE-SR4", "Multimode fibre, 850 nm, 8 fibres (MPO-12)", "40 Gb/s", "100 m (OM3), 150 m (OM4)", "Breaks out into 4 x 10GBASE-SR"),
    ("40GBASE-LR4", "Single-mode fibre, 4 CWDM wavelengths around 1310 nm", "40 Gb/s", "10 km", "Duplex LC"),
    ("40GBASE-ER4", "Single-mode fibre, 4 CWDM wavelengths around 1310 nm", "40 Gb/s", "40 km", "30 km on ordinary links, 40 km on engineered ones"),
    ("50GBASE-CR", "Twinax direct-attach cable (SFP56)", "50 Gb/s", "3 m", ""),
    ("50GBASE-SR", "Multimode fibre, 850 nm", "50 Gb/s", "70 m (OM3), 100 m (OM4)", ""),
    ("50GBASE-FR", "Single-mode fibre, 1310 nm", "50 Gb/s", "2 km", ""),
    ("50GBASE-LR", "Single-mode fibre, 1310 nm", "50 Gb/s", "10 km", ""),
    ("100GBASE-CR4", "Twinax direct-attach cable (QSFP28), 4 lanes", "100 Gb/s", "5 m", ""),
    ("100GBASE-SR10", "Multimode fibre, 850 nm, 20 fibres (MPO-24)", "100 Gb/s", "100 m (OM3), 150 m (OM4)", "First generation, 10 x 10 Gb/s lanes"),
    ("100GBASE-SR4", "Multimode fibre, 850 nm, 8 fibres (MPO-12)", "100 Gb/s", "70 m (OM3), 100 m (OM4)", "Breaks out into 4 x 25GBASE-SR"),
    ("100G-PSM4", "Single-mode fibre, 1310 nm, 8 fibres (MPO-12)", "100 Gb/s", "500 m", "Multi-source agreement, not an IEEE standard"),
    ("100G-CWDM4", "Single-mode fibre, 4 CWDM wavelengths around 1310 nm", "100 Gb/s", "2 km", "Multi-source agreement, not an IEEE standard; duplex LC"),
    ("100GBASE-DR", "Single-mode fibre, 1310 nm, one wavelength", "100 Gb/s", "500 m", "Interoperates with one lane of 400GBASE-DR4"),
    ("100GBASE-FR1", "Single-mode fibre, 1310 nm, one wavelength", "100 Gb/s", "2 km", ""),
    ("100GBASE-LR1", "Single-mode fibre, 1310 nm, one wavelength", "100 Gb/s", "10 km", ""),
    ("100GBASE-LR4", "Single-mode fibre, 4 LAN-WDM wavelengths around 1310 nm", "100 Gb/s", "10 km", "Duplex LC"),
    ("100GBASE-ER4", "Single-mode fibre, 4 LAN-WDM wavelengths around 1310 nm", "100 Gb/s", "40 km", "30 km on ordinary links, 40 km on engineered ones"),
    ("100GBASE-ZR", "Single-mode fibre, coherent DWDM, 1550 nm band", "100 Gb/s", "80 km", "Over an amplified DWDM line system"),
    ("200GBASE-SR4", "Multimode fibre, 850 nm, 8 fibres (MPO-12)", "200 Gb/s", "70 m (OM3), 100 m (OM4)", ""),
    ("200GBASE-DR4", "Single-mode fibre, 1310 nm, 8 fibres (MPO-12)", "200 Gb/s", "500 m", ""),
    ("200GBASE-FR4", "Single-mode fibre, 4 CWDM wavelengths around 1310 nm", "200 Gb/s", "2 km", "Duplex LC"),
    ("200GBASE-LR4", "Single-mode fibre, 4 LAN-WDM wavelengths around 1310 nm", "200 Gb/s", "10 km", "Duplex LC"),
    ("400GBASE-SR16", "Multimode fibre, 850 nm, 32 fibres (MPO-32)", "400 Gb/s", "70 m (OM3), 100 m (OM4)", "Rarely deployed"),
    ("400GBASE-SR8", "Multimode fibre, 850 nm, 16 fibres (MPO-16)", "400 Gb/s", "70 m (OM3), 100 m (OM4)", ""),
    ("400GBASE-SR4.2", "Multimode fibre, 850 and 910 nm, 8 fibres (MPO-12)", "400 Gb/s", "70 m (OM3), 100 m (OM4), 150 m (OM5)", "Bidirectional; reuses 40G and 100G SR4 fibre plant"),
    ("400GBASE-DR4", "Single-mode fibre, 1310 nm, 8 fibres (MPO-12)", "400 Gb/s", "500 m", "Breaks out into 4 x 100GBASE-DR"),
    ("400GBASE-FR4", "Single-mode fibre, 4 CWDM wavelengths around 1310 nm", "400 Gb/s", "2 km", "Duplex LC"),
    ("400GBASE-FR8", "Single-mode fibre, 8 LAN-WDM wavelengths around 1310 nm", "400 Gb/s", "2 km", "Duplex LC"),
    ("400GBASE-LR4-6", "Single-mode fibre, 4 CWDM wavelengths around 1310 nm", "400 Gb/s", "6 km", "Duplex LC"),
    ("400GBASE-LR8", "Single-mode fibre, 8 LAN-WDM wavelengths around 1310 nm", "400 Gb/s", "10 km", "Duplex LC"),
    ("400GBASE-ER8", "Single-mode fibre, 8 LAN-WDM wavelengths around 1310 nm", "400 Gb/s", "40 km", "Duplex LC"),
    ("400GBASE-ZR", "Single-mode fibre, coherent DWDM, 1550 nm band", "400 Gb/s", "80 km", "Over an amplified DWDM line system; the OIF 400ZR optics it derives from reach about 120 km"),
];

/// Ethernet physical layers from 10BASE5 to 400GBASE-ZR, including the
/// common optics, with medium and reach.
pub fn ethernet_standards() -> Vec<CableEntry> {
    STANDARDS
        .iter()
        .map(
            |(standard, medium, speed, max_distance, notes)| CableEntry {
                standard: (*standard).to_owned(),
                medium: (*medium).to_owned(),
                speed: (*speed).to_owned(),
                max_distance: (*max_distance).to_owned(),
                notes: (*notes).to_owned(),
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reach(standard: &str) -> String {
        ethernet_standards()
            .into_iter()
            .find(|entry| entry.standard == standard)
            .map(|entry| entry.max_distance)
            .unwrap()
    }

    #[test]
    fn copper_reaches() {
        for standard in [
            "10BASE-T",
            "100BASE-TX",
            "1000BASE-T",
            "2.5GBASE-T",
            "5GBASE-T",
            "10GBASE-T",
        ] {
            assert_eq!(reach(standard), "100 m", "{standard}");
        }
        assert_eq!(reach("25GBASE-T"), "30 m");
        assert_eq!(reach("40GBASE-T"), "30 m");
        assert_eq!(reach("10BASE5"), "500 m");
        assert_eq!(reach("10BASE2"), "185 m");
    }

    #[test]
    fn fibre_reaches() {
        for (standard, distance) in [
            ("1000BASE-SX", "550 m (OM2 and better), 275 m (OM1)"),
            ("1000BASE-LX", "5 km"),
            ("10GBASE-SR", "300 m (OM3), 400 m (OM4)"),
            ("10GBASE-LRM", "220 m"),
            ("10GBASE-LR", "10 km"),
            ("10GBASE-ER", "40 km"),
            ("25GBASE-SR", "70 m (OM3), 100 m (OM4)"),
            ("40GBASE-SR4", "100 m (OM3), 150 m (OM4)"),
            ("40GBASE-LR4", "10 km"),
            ("100GBASE-SR4", "70 m (OM3), 100 m (OM4)"),
            ("100GBASE-LR4", "10 km"),
            ("100GBASE-DR", "500 m"),
            ("400GBASE-DR4", "500 m"),
            ("400GBASE-FR4", "2 km"),
            ("400GBASE-LR8", "10 km"),
            ("400GBASE-ZR", "80 km"),
        ] {
            assert_eq!(reach(standard), distance, "{standard}");
        }
    }

    #[test]
    fn the_table_is_complete_and_tidy() {
        let table = ethernet_standards();
        assert_eq!(table.first().unwrap().standard, "10BASE5");
        assert_eq!(table.last().unwrap().standard, "400GBASE-ZR");
        assert!(table.len() >= 60);
        let speeds = [
            "10 Mb/s", "100 Mb/s", "1 Gb/s", "2.5 Gb/s", "5 Gb/s", "10 Gb/s", "25 Gb/s", "40 Gb/s",
            "50 Gb/s", "100 Gb/s", "200 Gb/s", "400 Gb/s",
        ];
        let mut last_speed = 0;
        for entry in &table {
            let speed = speeds
                .iter()
                .position(|speed| *speed == entry.speed)
                .unwrap_or_else(|| panic!("speed {}", entry.speed));
            assert!(speed >= last_speed, "{} is out of order", entry.standard);
            last_speed = speed;
            assert!(
                !entry.medium.is_empty()
                    && !entry.max_distance.is_empty()
                    && !entry.notes.ends_with('.'),
                "{}",
                entry.standard
            );
            assert_eq!(
                table
                    .iter()
                    .filter(|other| other.standard == entry.standard)
                    .count(),
                1,
                "{}",
                entry.standard
            );
        }
    }
}
