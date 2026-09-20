//! Vendor-neutral typed decision assessment contracts.
use crate::{
    assessment::{ExecutionClass, TaskFactValue},
    contracts::{EngineError, Result},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_QUESTIONS: usize = 32;
const MAX_OPTIONS: usize = 32;
const MAX_STATE_VALUES: usize = 64;
const PROBABILITY_TOLERANCE: f64 = 1e-9;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequest {
    pub question_set_version: String,
    pub state: DecisionState,
    pub questions: Vec<DecisionQuestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionState {
    pub version: String,
    pub values: BTreeMap<String, TaskFactValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionQuestion {
    pub id: String,
    pub kind: DecisionQuestionKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DecisionQuestionKind {
    Choice { options: Vec<String> },
    Score { minimum: f64, maximum: f64 },
    BooleanProbability,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionAssessment {
    pub question_set_version: String,
    pub adapter_id: String,
    pub actual_model: String,
    pub signals: Vec<DecisionSignal>,
    #[serde(default)]
    pub actual_cost: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionSignal {
    pub question_id: String,
    pub value: DecisionValue,
    pub probabilities: BTreeMap<String, f64>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum DecisionValue {
    Choice(String),
    Score(f64),
    Boolean(bool),
}

#[async_trait]
pub trait DecisionModel: Send + Sync {
    async fn assess(&self, request: DecisionRequest) -> Result<DecisionAssessment>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionConfig {
    pub question_set_version: String,
    pub questions: Vec<DecisionQuestion>,
    pub policy: DecisionPolicy,
    #[serde(default)]
    pub max_cost: u64,
}

impl DecisionConfig {
    pub fn validate(&self) -> Result<()> {
        let request = DecisionRequest {
            question_set_version: self.question_set_version.clone(),
            state: DecisionState {
                version: "config-validation-v1".into(),
                values: BTreeMap::new(),
            },
            questions: self.questions.clone(),
        };
        request.validate()?;
        self.policy.validate_for(&request)?;
        if self.max_cost > i64::MAX as u64 {
            return Err(invalid("decision.max_cost", "cost exceeds ledger range"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionPolicy {
    pub version: String,
    pub rules: Vec<DecisionPolicyRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionPolicyRule {
    pub question_id: String,
    pub outcome: String,
    pub minimum_probability: f64,
    pub execution_class: ExecutionClass,
    pub needs_clarification: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecommendation {
    pub policy_version: String,
    pub execution_class: ExecutionClass,
    pub needs_clarification: bool,
    pub reasons: Vec<String>,
}

impl DecisionPolicy {
    pub fn validate(&self) -> Result<()> {
        validate_nonempty("policy.version", &self.version)?;
        let mut keys = BTreeSet::new();
        for rule in &self.rules {
            validate_nonempty("policy.rule.question_id", &rule.question_id)?;
            validate_nonempty("policy.rule.outcome", &rule.outcome)?;
            if !rule.minimum_probability.is_finite()
                || !(0.0..=1.0).contains(&rule.minimum_probability)
            {
                return Err(invalid(
                    "policy.rule.minimum_probability",
                    "threshold must be between zero and one",
                ));
            }
            if !keys.insert((&rule.question_id, &rule.outcome)) {
                return Err(invalid(
                    "policy.rules",
                    "duplicate question and outcome rule",
                ));
            }
        }
        Ok(())
    }

    pub fn validate_for(&self, request: &DecisionRequest) -> Result<()> {
        self.validate()?;
        let questions = request
            .questions
            .iter()
            .map(|question| (question.id.as_str(), &question.kind))
            .collect::<BTreeMap<_, _>>();
        for rule in &self.rules {
            let kind = questions.get(rule.question_id.as_str()).ok_or_else(|| {
                invalid(
                    "policy.rule.question_id",
                    "policy references an unknown question",
                )
            })?;
            let valid_outcome = match kind {
                DecisionQuestionKind::Choice { options } => options.contains(&rule.outcome),
                DecisionQuestionKind::BooleanProbability => {
                    matches!(rule.outcome.as_str(), "true" | "false")
                }
                DecisionQuestionKind::Score { minimum, maximum } => {
                    rule.outcome.parse::<f64>().ok().is_some_and(|value| {
                        value.is_finite() && (*minimum..=*maximum).contains(&value)
                    })
                }
            };
            if !valid_outcome {
                return Err(invalid(
                    "policy.rule.outcome",
                    "policy outcome is outside the question domain",
                ));
            }
        }
        Ok(())
    }

    pub fn apply(
        &self,
        request: &DecisionRequest,
        assessment: &DecisionAssessment,
    ) -> Result<DecisionRecommendation> {
        request.validate()?;
        assessment.validate_for(request)?;
        self.validate_for(request)?;
        let mut execution_class = ExecutionClass::Simple;
        let mut needs_clarification = false;
        let mut reasons = Vec::new();
        for rule in &self.rules {
            let probability = assessment
                .signals
                .iter()
                .find(|signal| signal.question_id == rule.question_id)
                .and_then(|signal| signal.probabilities.get(&rule.outcome))
                .copied();
            if probability.is_some_and(|value| value >= rule.minimum_probability) {
                execution_class = execution_class.max(rule.execution_class);
                needs_clarification |= rule.needs_clarification;
                reasons.push(format!(
                    "{}:{}>={:.3}",
                    rule.question_id, rule.outcome, rule.minimum_probability
                ));
            }
        }
        Ok(DecisionRecommendation {
            policy_version: self.version.clone(),
            execution_class,
            needs_clarification,
            reasons,
        })
    }
}

impl DecisionRequest {
    pub fn validate(&self) -> Result<()> {
        validate_nonempty("question_set_version", &self.question_set_version)?;
        validate_nonempty("state.version", &self.state.version)?;
        if self.state.values.len() > MAX_STATE_VALUES {
            return Err(invalid("state.values", "too many state values"));
        }
        if self.questions.is_empty() || self.questions.len() > MAX_QUESTIONS {
            return Err(invalid("questions", "invalid question count"));
        }
        let mut ids = BTreeSet::new();
        for question in &self.questions {
            validate_nonempty("question.id", &question.id)?;
            if !ids.insert(&question.id) {
                return Err(invalid("question.id", "duplicate question ID"));
            }
            validate_question_kind(&question.kind)?;
        }
        Ok(())
    }
}

impl DecisionAssessment {
    pub fn validate_for(&self, request: &DecisionRequest) -> Result<()> {
        request.validate()?;
        validate_nonempty("adapter_id", &self.adapter_id)?;
        validate_nonempty("actual_model", &self.actual_model)?;
        if self.actual_cost.is_some_and(|cost| cost > i64::MAX as u64) {
            return Err(invalid("actual_cost", "cost exceeds ledger range"));
        }
        if self.question_set_version != request.question_set_version {
            return Err(invalid(
                "question_set_version",
                "assessment version does not match request",
            ));
        }
        if self.signals.len() != request.questions.len() {
            return Err(invalid(
                "signals",
                "assessment must answer every question exactly once",
            ));
        }
        let questions = request
            .questions
            .iter()
            .map(|question| (question.id.as_str(), &question.kind))
            .collect::<BTreeMap<_, _>>();
        let mut seen = BTreeSet::new();
        for signal in &self.signals {
            if !seen.insert(&signal.question_id) {
                return Err(invalid(
                    "signals.question_id",
                    "duplicate signal question ID",
                ));
            }
            let kind = questions.get(signal.question_id.as_str()).ok_or_else(|| {
                invalid(
                    "signals.question_id",
                    "signal references an unknown question",
                )
            })?;
            validate_signal(signal, kind)?;
        }
        if seen.len() != questions.len() {
            return Err(invalid("signals", "assessment omitted a question"));
        }
        Ok(())
    }
}

pub struct FakeDecisionModel {
    assessment: DecisionAssessment,
}

impl FakeDecisionModel {
    pub fn new(assessment: DecisionAssessment) -> Self {
        Self { assessment }
    }
}

#[async_trait]
impl DecisionModel for FakeDecisionModel {
    async fn assess(&self, request: DecisionRequest) -> Result<DecisionAssessment> {
        self.assessment.validate_for(&request)?;
        Ok(self.assessment.clone())
    }
}

fn validate_question_kind(kind: &DecisionQuestionKind) -> Result<()> {
    match kind {
        DecisionQuestionKind::Choice { options } => {
            if options.is_empty() || options.len() > MAX_OPTIONS {
                return Err(invalid("question.options", "invalid option count"));
            }
            let mut unique = BTreeSet::new();
            for option in options {
                validate_nonempty("question.options", option)?;
                if !unique.insert(option) {
                    return Err(invalid("question.options", "duplicate option"));
                }
            }
        }
        DecisionQuestionKind::Score { minimum, maximum } => {
            if !minimum.is_finite() || !maximum.is_finite() || minimum >= maximum {
                return Err(invalid("question.range", "invalid score range"));
            }
        }
        DecisionQuestionKind::BooleanProbability => {}
    }
    Ok(())
}

fn validate_signal(signal: &DecisionSignal, kind: &DecisionQuestionKind) -> Result<()> {
    if signal.probabilities.is_empty()
        || signal
            .probabilities
            .values()
            .any(|probability| !probability.is_finite() || !(0.0..=1.0).contains(probability))
        || (signal.probabilities.values().sum::<f64>() - 1.0).abs() > PROBABILITY_TOLERANCE
    {
        return Err(invalid(
            "signal.probabilities",
            "probabilities must sum to one",
        ));
    }
    let expected_keys = match kind {
        DecisionQuestionKind::Choice { options } => options.iter().cloned().collect(),
        DecisionQuestionKind::BooleanProbability => BTreeSet::from(["false".into(), "true".into()]),
        DecisionQuestionKind::Score { .. } => BTreeSet::new(),
    };
    if !expected_keys.is_empty()
        && signal
            .probabilities
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            != expected_keys
    {
        return Err(invalid(
            "signal.probabilities",
            "probability outcomes do not match the question domain",
        ));
    }
    if let Some(confidence) = signal.confidence {
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(invalid(
                "signal.confidence",
                "confidence must be between zero and one",
            ));
        }
    }
    match (kind, &signal.value) {
        (DecisionQuestionKind::Choice { options }, DecisionValue::Choice(value))
            if options.iter().any(|option| option == value)
                && signal.probabilities.contains_key(value) => {}
        (DecisionQuestionKind::Score { minimum, maximum }, DecisionValue::Score(value))
            if value.is_finite() && (*minimum..=*maximum).contains(value) => {}
        (DecisionQuestionKind::BooleanProbability, DecisionValue::Boolean(value))
            if signal
                .probabilities
                .contains_key(if *value { "true" } else { "false" }) => {}
        _ => {
            return Err(invalid(
                "signal.value",
                "value does not match question kind or domain",
            ))
        }
    }
    Ok(())
}

fn validate_nonempty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(invalid(field, "value must be nonempty"))
    } else {
        Ok(())
    }
}

fn invalid(field: &str, message: &str) -> EngineError {
    EngineError::new("decision", message).details(serde_json::json!({"field": field}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> DecisionRequest {
        DecisionRequest {
            question_set_version: "demand-v1".into(),
            state: DecisionState {
                version: "facts-v1".into(),
                values: BTreeMap::new(),
            },
            questions: vec![DecisionQuestion {
                id: "reasoning_need".into(),
                kind: DecisionQuestionKind::Choice {
                    options: vec!["direct".into(), "multi_step".into()],
                },
            }],
        }
    }

    fn assessment(probabilities: BTreeMap<String, f64>) -> DecisionAssessment {
        DecisionAssessment {
            question_set_version: "demand-v1".into(),
            adapter_id: "fake-v1".into(),
            actual_model: "fake-model-v1".into(),
            signals: vec![DecisionSignal {
                question_id: "reasoning_need".into(),
                value: DecisionValue::Choice("multi_step".into()),
                probabilities,
                confidence: Some(0.9),
            }],
            actual_cost: None,
        }
    }

    #[tokio::test]
    async fn fake_adapter_returns_only_a_validated_assessment() {
        let model = FakeDecisionModel::new(assessment(BTreeMap::from([
            ("direct".into(), 0.2),
            ("multi_step".into(), 0.8),
        ])));
        let result = model.assess(request()).await.unwrap();
        assert_eq!(result.signals[0].question_id, "reasoning_need");
    }

    #[test]
    fn probabilities_must_be_complete_and_bounded() {
        let error = assessment(BTreeMap::from([("multi_step".into(), 0.8)]))
            .validate_for(&request())
            .unwrap_err();
        assert_eq!(error.kind, "decision");
    }

    #[test]
    fn choice_value_must_be_declared_by_question() {
        let mut invalid = assessment(BTreeMap::from([
            ("direct".into(), 0.2),
            ("multi_step".into(), 0.8),
        ]));
        invalid.signals[0].value = DecisionValue::Choice("unsupported".into());
        assert!(invalid.validate_for(&request()).is_err());
    }

    #[test]
    fn probability_outcomes_must_match_the_declared_choice_domain() {
        let invalid = assessment(BTreeMap::from([
            ("direct".into(), 0.1),
            ("multi_step".into(), 0.8),
            ("unsupported".into(), 0.1),
        ]));
        assert!(invalid.validate_for(&request()).is_err());
    }

    #[test]
    fn policy_maps_probability_to_the_highest_class_deterministically() {
        let assessment = assessment(BTreeMap::from([
            ("direct".into(), 0.2),
            ("multi_step".into(), 0.8),
        ]));
        let policy = DecisionPolicy {
            version: "demand-policy-v1".into(),
            rules: vec![DecisionPolicyRule {
                question_id: "reasoning_need".into(),
                outcome: "multi_step".into(),
                minimum_probability: 0.75,
                execution_class: ExecutionClass::Hard,
                needs_clarification: false,
            }],
        };
        let recommendation = policy.apply(&request(), &assessment).unwrap();
        assert_eq!(recommendation.execution_class, ExecutionClass::Hard);
        assert_eq!(
            recommendation.reasons,
            vec!["reasoning_need:multi_step>=0.750"]
        );
    }

    #[test]
    fn policy_rejects_duplicate_rules_and_invalid_thresholds() {
        let policy = DecisionPolicy {
            version: "policy-v1".into(),
            rules: vec![DecisionPolicyRule {
                question_id: "reasoning_need".into(),
                outcome: "multi_step".into(),
                minimum_probability: 1.1,
                execution_class: ExecutionClass::Medium,
                needs_clarification: true,
            }],
        };
        assert!(policy.validate().is_err());
    }
}
