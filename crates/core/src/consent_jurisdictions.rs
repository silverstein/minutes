//! Informational recording-consent reminders by US jurisdiction.
//!
//! This small table may be out of date and is not legal advice. Each row links
//! to a primary state source so users can confirm the rules that apply to them.

use crate::markdown::ConsentBasis;

/// One informational jurisdiction reminder backed by a primary source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsentJurisdiction {
    /// ISO-3166-2-style jurisdiction code.
    pub code: &'static str,
    /// Short, deliberately qualified reminder shown to the user.
    pub note: &'static str,
    /// Primary state source for the reminder.
    pub source_url: &'static str,
    /// Consent basis Minutes recommends the user consider recording.
    pub recommended_basis: ConsentBasis,
}

const ALL_PARTY_REMINDERS: &[ConsentJurisdiction] = &[
    ConsentJurisdiction {
        code: "US-CA",
        note: "California is commonly treated as requiring all-party consent for confidential communications.",
        source_url: "https://leginfo.legislature.ca.gov/faces/codes_displaySection.xhtml?lawCode=PEN&sectionNum=632.",
        recommended_basis: ConsentBasis::VerbalAllParties,
    },
    ConsentJurisdiction {
        code: "US-FL",
        note: "Florida is commonly treated as requiring prior consent from all parties for covered communications.",
        source_url: "https://www.leg.state.fl.us/statutes/index.cfm?App_mode=Display_Statute&URL=0900-0999/0934/Sections/0934.03.html",
        recommended_basis: ConsentBasis::VerbalAllParties,
    },
    ConsentJurisdiction {
        code: "US-IL",
        note: "Illinois requires all-party consent for covered private conversations recorded surreptitiously.",
        source_url: "https://www.ilga.gov/documents/legislation/ilcs/documents/072000050K14-2.htm",
        recommended_basis: ConsentBasis::VerbalAllParties,
    },
    ConsentJurisdiction {
        code: "US-MD",
        note: "Maryland is commonly treated as requiring prior consent from all parties for covered communications.",
        source_url: "https://mgaleg.maryland.gov/mgawebsite/Laws/StatuteText?article=gcj&section=10-402&enactments=false",
        recommended_basis: ConsentBasis::VerbalAllParties,
    },
    ConsentJurisdiction {
        code: "US-MA",
        note: "Massachusetts generally prohibits secret recording without prior authority from all parties.",
        source_url: "https://malegislature.gov/Laws/GeneralLaws/PartIV/TitleI/Chapter272/Section99",
        recommended_basis: ConsentBasis::VerbalAllParties,
    },
    ConsentJurisdiction {
        code: "US-PA",
        note: "Pennsylvania is commonly treated as requiring prior consent from all parties for covered communications.",
        source_url: "https://www.legis.state.pa.us/WU01/LI/LI/CT/HTM/18/00.057.004.000..HTM",
        recommended_basis: ConsentBasis::VerbalAllParties,
    },
    ConsentJurisdiction {
        code: "US-WA",
        note: "Washington generally requires consent from all participants for covered private communications and conversations.",
        source_url: "https://app.leg.wa.gov/rcw/default.aspx?cite=9.73.030",
        recommended_basis: ConsentBasis::VerbalAllParties,
    },
];

/// Look up an informational all-party reminder by jurisdiction code.
pub fn lookup(code: &str) -> Option<&'static ConsentJurisdiction> {
    ALL_PARTY_REMINDERS
        .iter()
        .find(|row| row.code.eq_ignore_ascii_case(code.trim()))
}

/// Return the built-in informational reminder rows.
pub fn all() -> &'static [ConsentJurisdiction] {
    ALL_PARTY_REMINDERS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_is_case_insensitive_and_primary_sources_are_https() {
        let ca = lookup("us-ca").expect("California reminder");
        assert_eq!(ca.recommended_basis, ConsentBasis::VerbalAllParties);
        assert!(all()
            .iter()
            .all(|row| row.source_url.starts_with("https://")));
    }
}
