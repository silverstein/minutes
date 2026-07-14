use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum MeetingMode {
    Sales,
    Discovery,
    Interview,
    Negotiation,
    DifficultConversation,
    Decision,
    #[default]
    Generic,
}

impl MeetingMode {
    pub const ALL: [&'static str; 7] = [
        "sales",
        "discovery",
        "interview",
        "negotiation",
        "difficult-conversation",
        "decision",
        "generic",
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sales => "sales",
            Self::Discovery => "discovery",
            Self::Interview => "interview",
            Self::Negotiation => "negotiation",
            Self::DifficultConversation => "difficult-conversation",
            Self::Decision => "decision",
            Self::Generic => "generic",
        }
    }

    pub fn policy(self) -> MeetingModePolicy {
        let (minimum_confidence, minimum_interval_ms, tone) = match self {
            Self::Sales => (
                65,
                4_000,
                "concise, commercially aware, and useful without sounding scripted",
            ),
            Self::Discovery => (
                55,
                6_000,
                "curious and open-ended; favor learning over premature solutions",
            ),
            Self::Interview => (
                70,
                8_000,
                "neutral and evidence-seeking; avoid leading the candidate",
            ),
            Self::Negotiation => (
                75,
                12_000,
                "calm, precise, and leverage-aware; never manufacture urgency",
            ),
            Self::DifficultConversation => (
                75,
                15_000,
                "empathetic, de-escalating, and direct without assigning motives",
            ),
            Self::Decision => (
                65,
                5_000,
                "crisp and outcome-oriented; surface tradeoffs, ownership, and closure",
            ),
            Self::Generic => (
                0,
                0,
                "brief, grounded, and low-pressure; intervene only when clearly useful",
            ),
        };
        MeetingModePolicy {
            mode: self,
            minimum_confidence,
            minimum_interval_ms,
            tone,
        }
    }
}

impl fmt::Display for MeetingMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for MeetingMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "sales" => Ok(Self::Sales),
            "discovery" => Ok(Self::Discovery),
            "interview" => Ok(Self::Interview),
            "negotiation" => Ok(Self::Negotiation),
            "difficult-conversation" | "difficult_conversation" => Ok(Self::DifficultConversation),
            "decision" => Ok(Self::Decision),
            "generic" | "" => Ok(Self::Generic),
            other => Err(format!(
                "unsupported copilot mode '{other}'; use {}",
                Self::ALL.join(", ")
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpportunityKind {
    Pain,
    Objection,
    NextStep,
    Evidence,
    Decision,
    Leverage,
    Rapport,
    Clarity,
    Safety,
    #[default]
    General,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetingModePolicy {
    pub mode: MeetingMode,
    pub minimum_confidence: u8,
    pub minimum_interval_ms: u64,
    pub tone: &'static str,
}

impl MeetingModePolicy {
    pub fn allows(self, opportunity: OpportunityKind) -> bool {
        use OpportunityKind::*;
        opportunity == General
            || match self.mode {
                MeetingMode::Sales => {
                    matches!(
                        opportunity,
                        Pain | Objection | NextStep | Decision | Clarity
                    )
                }
                MeetingMode::Discovery => {
                    matches!(opportunity, Pain | Objection | Evidence | Rapport | Clarity)
                }
                MeetingMode::Interview => {
                    matches!(opportunity, Evidence | Rapport | Clarity | Safety)
                }
                MeetingMode::Negotiation => {
                    matches!(
                        opportunity,
                        Objection | Decision | Leverage | Clarity | Safety
                    )
                }
                MeetingMode::DifficultConversation => {
                    matches!(opportunity, Rapport | Clarity | Safety | NextStep)
                }
                MeetingMode::Decision => {
                    matches!(
                        opportunity,
                        NextStep | Evidence | Decision | Clarity | Safety
                    )
                }
                MeetingMode::Generic => true,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_and_expose_distinct_policies() {
        assert_eq!(
            "difficult-conversation".parse(),
            Ok(MeetingMode::DifficultConversation)
        );
        assert!(MeetingMode::Discovery
            .policy()
            .allows(OpportunityKind::Pain));
        assert!(!MeetingMode::Decision.policy().allows(OpportunityKind::Pain));
        assert!(
            MeetingMode::Negotiation.policy().minimum_interval_ms
                > MeetingMode::Sales.policy().minimum_interval_ms
        );
    }
}
