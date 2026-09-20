//! Deterministic task facts and execution-class assessment.
use crate::contracts::{EngineError, Result};
use crate::decision::{DecisionAssessment, DecisionRecommendation};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_FACTS: usize = 64;
const MAX_RULES: usize = 128;
const MAX_STRING_BYTES: usize = 256;
const MAX_SET_ITEMS: usize = 64;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionClass {
    #[default]
    Simple,
    Medium,
    Hard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TaskFactValue {
    Boolean(bool),
    Integer(u64),
    String(String),
    StringSet(BTreeSet<String>),
}

impl TaskFactValue {
    fn validate(&self, field: &str) -> Result<()> {
        match self {
            Self::Boolean(_) | Self::Integer(_) => Ok(()),
            Self::String(value) => validate_string(field, value),
            Self::StringSet(values) => {
                if values.len() > MAX_SET_ITEMS {
                    return Err(invalid(field, "fact string set is too large"));
                }
                for value in values {
                    validate_string(field, value)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskFacts {
    pub version: String,
    pub values: BTreeMap<String, TaskFactValue>,
}

impl TaskFacts {
    pub fn validate(&self) -> Result<()> {
        validate_string("facts.version", &self.version)?;
        if self.values.len() > MAX_FACTS {
            return Err(invalid("facts.values", "too many task facts"));
        }
        for (name, value) in &self.values {
            validate_string("facts.values key", name)?;
            value.validate(&format!("facts.values.{name}"))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOperator {
    Eq,
    Gte,
    Gt,
    Contains,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentRule {
    pub id: String,
    pub fact: String,
    pub op: RuleOperator,
    pub value: TaskFactValue,
    pub minimum_execution_class: ExecutionClass,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSet {
    pub version: String,
    pub rules: Vec<AssessmentRule>,
}

impl RuleSet {
    pub fn validate(&self) -> Result<()> {
        validate_string("rules.version", &self.version)?;
        if self.rules.len() > MAX_RULES {
            return Err(invalid("rules", "too many assessment rules"));
        }
        let mut ids = BTreeSet::new();
        for (index, rule) in self.rules.iter().enumerate() {
            let field = format!("rules[{index}]");
            validate_string(&format!("{field}.id"), &rule.id)?;
            validate_string(&format!("{field}.fact"), &rule.fact)?;
            if !ids.insert(&rule.id) {
                return Err(invalid(&format!("{field}.id"), "duplicate rule ID"));
            }
            rule.value.validate(&format!("{field}.value"))?;
            validate_operator(&field, rule.op, &rule.value)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleStatus {
    Matched,
    NotMatched,
    NotObserved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleResult {
    pub rule_id: String,
    pub fact: String,
    pub status: RuleStatus,
    pub observed: Option<TaskFactValue>,
    pub minimum_execution_class: ExecutionClass,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleAssessment {
    pub facts_version: String,
    pub rule_set_version: String,
    pub results: Vec<RuleResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAssessment {
    pub execution_class: ExecutionClass,
    pub facts_version: String,
    pub rule_set_version: String,
    pub matched_rule_ids: Vec<String>,
    pub not_observed_facts: Vec<String>,
    pub reasons: Vec<String>,
    #[serde(default)]
    pub decision_policy_version: Option<String>,
    #[serde(default)]
    pub needs_clarification: bool,
    #[serde(default)]
    pub decision: Option<DecisionAssessment>,
}

pub fn evaluate_rules(facts: &TaskFacts, rules: &RuleSet) -> Result<RuleAssessment> {
    facts.validate()?;
    rules.validate()?;
    let results = rules
        .rules
        .iter()
        .map(|rule| {
            let observed = facts.values.get(&rule.fact).cloned();
            let status = match &observed {
                None => RuleStatus::NotObserved,
                Some(value) if matches_rule(value, rule.op, &rule.value) => RuleStatus::Matched,
                Some(_) => RuleStatus::NotMatched,
            };
            RuleResult {
                rule_id: rule.id.clone(),
                fact: rule.fact.clone(),
                status,
                observed,
                minimum_execution_class: rule.minimum_execution_class,
            }
        })
        .collect();
    Ok(RuleAssessment {
        facts_version: facts.version.clone(),
        rule_set_version: rules.version.clone(),
        results,
    })
}

pub fn merge_assessment(
    host_minimum: ExecutionClass,
    rules: &RuleAssessment,
    additional_minimum: Option<ExecutionClass>,
) -> TaskAssessment {
    merge_assessment_with_evidence(host_minimum, rules, additional_minimum, None, None)
}

pub fn merge_assessment_with_decision(
    host_minimum: ExecutionClass,
    rules: &RuleAssessment,
    additional_minimum: Option<ExecutionClass>,
    decision: Option<&DecisionRecommendation>,
) -> TaskAssessment {
    merge_assessment_with_evidence(host_minimum, rules, additional_minimum, decision, None)
}

pub fn merge_assessment_with_evidence(
    host_minimum: ExecutionClass,
    rules: &RuleAssessment,
    additional_minimum: Option<ExecutionClass>,
    decision: Option<&DecisionRecommendation>,
    decision_evidence: Option<&DecisionAssessment>,
) -> TaskAssessment {
    let mut execution_class = host_minimum;
    let mut matched_rule_ids = Vec::new();
    let mut not_observed_facts = BTreeSet::new();
    let mut reasons = Vec::new();
    for result in &rules.results {
        match result.status {
            RuleStatus::Matched => {
                execution_class = execution_class.max(result.minimum_execution_class);
                matched_rule_ids.push(result.rule_id.clone());
                reasons.push(format!("rule:{}", result.rule_id));
            }
            RuleStatus::NotObserved => {
                not_observed_facts.insert(result.fact.clone());
            }
            RuleStatus::NotMatched => {}
        }
    }
    if let Some(minimum) = additional_minimum {
        execution_class = execution_class.max(minimum);
        reasons.push(format!("additional:{minimum:?}"));
    }
    if let Some(recommendation) = decision {
        execution_class = execution_class.max(recommendation.execution_class);
        reasons.extend(
            recommendation
                .reasons
                .iter()
                .map(|reason| format!("decision:{reason}")),
        );
    }
    TaskAssessment {
        execution_class,
        facts_version: rules.facts_version.clone(),
        rule_set_version: rules.rule_set_version.clone(),
        matched_rule_ids,
        not_observed_facts: not_observed_facts.into_iter().collect(),
        reasons,
        decision_policy_version: decision
            .map(|recommendation| recommendation.policy_version.clone()),
        needs_clarification: decision
            .is_some_and(|recommendation| recommendation.needs_clarification),
        decision: decision_evidence.cloned(),
    }
}

fn matches_rule(observed: &TaskFactValue, op: RuleOperator, expected: &TaskFactValue) -> bool {
    match op {
        RuleOperator::Eq => observed == expected,
        RuleOperator::Gte => {
            matches!((observed, expected), (TaskFactValue::Integer(a), TaskFactValue::Integer(b)) if a >= b)
        }
        RuleOperator::Gt => {
            matches!((observed, expected), (TaskFactValue::Integer(a), TaskFactValue::Integer(b)) if a > b)
        }
        RuleOperator::Contains => {
            matches!((observed, expected), (TaskFactValue::StringSet(values), TaskFactValue::String(value)) if values.contains(value))
        }
    }
}

fn validate_operator(field: &str, op: RuleOperator, value: &TaskFactValue) -> Result<()> {
    let valid = match op {
        RuleOperator::Eq => true,
        RuleOperator::Gte | RuleOperator::Gt => matches!(value, TaskFactValue::Integer(_)),
        RuleOperator::Contains => matches!(value, TaskFactValue::String(_)),
    };
    if valid {
        Ok(())
    } else {
        Err(invalid(field, "rule operator and value type do not match"))
    }
}

fn validate_string(field: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_STRING_BYTES {
        return Err(invalid(field, "string is empty or exceeds the byte limit"));
    }
    Ok(())
}

fn invalid(field: &str, message: &str) -> EngineError {
    EngineError::new("configuration", message).details(serde_json::json!({"field": field}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(values: &[(&str, TaskFactValue)]) -> TaskFacts {
        TaskFacts {
            version: "coding-facts-v1".into(),
            values: values
                .iter()
                .map(|(name, value)| ((*name).into(), value.clone()))
                .collect(),
        }
    }

    fn rule(
        id: &str,
        fact: &str,
        op: RuleOperator,
        value: TaskFactValue,
        class: ExecutionClass,
    ) -> AssessmentRule {
        AssessmentRule {
            id: id.into(),
            fact: fact.into(),
            op,
            value,
            minimum_execution_class: class,
        }
    }

    #[test]
    fn missing_fact_is_not_a_negative_match() {
        let assessment = evaluate_rules(
            &facts(&[]),
            &RuleSet {
                version: "coding-demand-v1".into(),
                rules: vec![rule(
                    "many-files",
                    "file_count",
                    RuleOperator::Gt,
                    TaskFactValue::Integer(5),
                    ExecutionClass::Hard,
                )],
            },
        )
        .unwrap();
        assert_eq!(assessment.results[0].status, RuleStatus::NotObserved);
    }

    #[test]
    fn rules_take_the_highest_class_and_host_floor() {
        let assessment = evaluate_rules(
            &facts(&[
                ("file_count", TaskFactValue::Integer(8)),
                (
                    "change_kinds",
                    TaskFactValue::StringSet(["migration".into()].into()),
                ),
            ]),
            &RuleSet {
                version: "coding-demand-v1".into(),
                rules: vec![
                    rule(
                        "many-files",
                        "file_count",
                        RuleOperator::Gt,
                        TaskFactValue::Integer(5),
                        ExecutionClass::Medium,
                    ),
                    rule(
                        "migration",
                        "change_kinds",
                        RuleOperator::Contains,
                        TaskFactValue::String("migration".into()),
                        ExecutionClass::Hard,
                    ),
                ],
            },
        )
        .unwrap();
        let merged = merge_assessment(ExecutionClass::Simple, &assessment, None);
        assert_eq!(merged.execution_class, ExecutionClass::Hard);
        assert_eq!(merged.matched_rule_ids, vec!["many-files", "migration"]);
    }

    #[test]
    fn invalid_operator_types_are_rejected() {
        let result = evaluate_rules(
            &facts(&[]),
            &RuleSet {
                version: "coding-demand-v1".into(),
                rules: vec![rule(
                    "bad",
                    "file_count",
                    RuleOperator::Gt,
                    TaskFactValue::String("5".into()),
                    ExecutionClass::Medium,
                )],
            },
        );
        assert_eq!(result.unwrap_err().kind, "configuration");
    }

    #[test]
    fn decision_recommendation_cannot_lower_hard_rule_floor() {
        let rules = evaluate_rules(
            &facts(&[("file_count", TaskFactValue::Integer(8))]),
            &RuleSet {
                version: "coding-demand-v1".into(),
                rules: vec![rule(
                    "many-files",
                    "file_count",
                    RuleOperator::Gt,
                    TaskFactValue::Integer(5),
                    ExecutionClass::Hard,
                )],
            },
        )
        .unwrap();
        let recommendation = DecisionRecommendation {
            policy_version: "demand-policy-v1".into(),
            execution_class: ExecutionClass::Simple,
            needs_clarification: true,
            reasons: vec!["low-confidence".into()],
        };
        let evidence = DecisionAssessment {
            question_set_version: "demand-v1".into(),
            adapter_id: "fake-v1".into(),
            actual_model: "fake-model-v1".into(),
            signals: vec![],
            actual_cost: None,
        };
        let merged = merge_assessment_with_evidence(
            ExecutionClass::Simple,
            &rules,
            None,
            Some(&recommendation),
            Some(&evidence),
        );
        assert_eq!(merged.execution_class, ExecutionClass::Hard);
        assert_eq!(
            merged.decision_policy_version.as_deref(),
            Some("demand-policy-v1")
        );
        assert!(merged.needs_clarification);
        assert_eq!(merged.decision.as_ref().unwrap().adapter_id, "fake-v1");
    }
}
