use super::{RateSnapshot, RequestUsage, Tokens};

pub(super) enum PriceEvidence {
    Complete,
    MissingTariff,
    MissingCache,
    MissingLifetime,
    Partial,
}

impl PriceEvidence {
    pub fn evaluate(tokens: &Tokens, rates: &Option<RateSnapshot>, partial: bool) -> Self {
        if partial {
            return Self::Partial;
        }
        if tokens.cache_write > 0 && tokens.write_12h.is_none() {
            return Self::MissingLifetime;
        }
        if !tokens.cached_is_known() || !tokens.writes_are_known() {
            return Self::MissingCache;
        }
        if rates.is_none() {
            return Self::MissingTariff;
        }
        Self::Complete
    }

    pub fn reason(&self) -> Option<String> {
        let reason = match self {
            Self::Complete => return None,
            Self::MissingTariff => "Model, service tier or long-context price is unavailable",
            Self::MissingCache => "Cache usage is not recorded",
            Self::MissingLifetime => "Cache-write lifetime is not recorded",
            Self::Partial => "Usage counter reset or aggregated requests; coverage is partial",
        };
        Some(reason.into())
    }
}

pub(super) enum PriceQuote {
    Known(f64),
    Unknown(Option<String>),
}

impl From<Result<f64, String>> for PriceQuote {
    fn from(result: Result<f64, String>) -> Self {
        match result {
            Ok(price) => Self::Known(price),
            Err(reason) => Self::Unknown(Some(reason)),
        }
    }
}

impl RequestUsage {
    pub(super) fn record_quote(&mut self, quote: PriceQuote) {
        let (cost, reason) = match quote {
            PriceQuote::Known(cost) if cost.is_finite() && cost >= 0.0 => (Some(cost), None),
            PriceQuote::Known(_) => (None, Some("The recorded price is invalid".into())),
            PriceQuote::Unknown(reason) => (None, reason),
        };
        self.cost_usd = cost;
        self.unpriced_reason = reason;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_transitions_keep_amount_and_unknown_reason_consistent() {
        let mut request = super::super::request(
            "fixture".into(), "fixture".into(), None, None, Tokens::default(), false);
        request.record_quote(PriceQuote::Known(0.25));
        assert_eq!(request.cost_usd, Some(0.25));
        assert!(request.unpriced_reason.is_none());
        request.record_quote(PriceQuote::Unknown(Some("missing tariff".into())));
        assert!(request.cost_usd.is_none());
        assert_eq!(request.unpriced_reason.as_deref(), Some("missing tariff"));
        for invalid in [f64::NAN, f64::INFINITY, -1.0] {
            request.record_quote(PriceQuote::Known(invalid));
            assert!(request.cost_usd.is_none());
            assert!(request.unpriced_reason.is_some());
        }
        request.record_quote(PriceQuote::Known(0.0));
        assert_eq!(request.cost_usd, Some(0.0));
        assert!(request.unpriced_reason.is_none());
    }
}
