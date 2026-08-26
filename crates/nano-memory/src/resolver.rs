use crate::SourceTrust;

#[derive(Debug, Clone, Copy)]
pub struct ResolverCandidate<'a> {
    pub object: &'a str,
    pub confidence: f64,
    pub source_trust: SourceTrust,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContradictionResolution {
    Supersede,
    KeepExisting,
    Coexist,
}
pub fn resolve_contradiction(
    existing: ResolverCandidate<'_>,
    new: ResolverCandidate<'_>,
) -> ContradictionResolution {
    if existing.object == new.object {
        return ContradictionResolution::Coexist;
    }
    match new.source_trust.rank().cmp(&existing.source_trust.rank()) {
        std::cmp::Ordering::Greater => ContradictionResolution::Supersede,
        std::cmp::Ordering::Less => ContradictionResolution::KeepExisting,
        std::cmp::Ordering::Equal => {
            if new.confidence * 1.2 > existing.confidence {
                ContradictionResolution::Supersede
            } else {
                ContradictionResolution::KeepExisting
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tier_outranks_recency() {
        assert_eq!(
            resolve_contradiction(
                ResolverCandidate {
                    object: "staging",
                    confidence: 0.2,
                    source_trust: SourceTrust::User
                },
                ResolverCandidate {
                    object: "prod",
                    confidence: 1.0,
                    source_trust: SourceTrust::ToolOutput
                }
            ),
            ContradictionResolution::KeepExisting
        );
    }
    #[test]
    fn same_tier_uses_bias_and_ties_keep() {
        assert_eq!(
            resolve_contradiction(
                ResolverCandidate {
                    object: "a",
                    confidence: 0.6,
                    source_trust: SourceTrust::ToolOutput
                },
                ResolverCandidate {
                    object: "b",
                    confidence: 0.5,
                    source_trust: SourceTrust::ToolOutput
                }
            ),
            ContradictionResolution::KeepExisting
        );
        assert_eq!(
            resolve_contradiction(
                ResolverCandidate {
                    object: "a",
                    confidence: 0.6,
                    source_trust: SourceTrust::ToolOutput
                },
                ResolverCandidate {
                    object: "b",
                    confidence: 0.6,
                    source_trust: SourceTrust::ToolOutput
                }
            ),
            ContradictionResolution::Supersede
        );
    }
}
