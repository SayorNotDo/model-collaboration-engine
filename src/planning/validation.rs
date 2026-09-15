use crate::contracts::{
    digest, EffectivePlan, EngineError, PlanChoice, PlannerProposal, Result, RoleProfile, Strategy,
    SubmissionSpec, TaskType,
};
use std::collections::BTreeMap;

pub(crate) fn validate_plan(
    submission: &SubmissionSpec,
    proposal: Option<(&str, &PlannerProposal)>,
    fallback: Option<&PlanChoice>,
) -> Result<EffectivePlan> {
    let bad = |message| EngineError::new("plan_validation", message);
    let mut acceptance = submission.acceptance.clone();
    let mut constraints = submission.constraints.clone();
    let choice = if let Some((_, p)) = proposal {
        if p.proposal_version != 1
            || !p.classification_confidence.is_finite()
            || !(0.0..=1.0).contains(&p.classification_confidence)
            || p.reason.len() > 4096
            || p.required_capabilities
                .iter()
                .any(|c| !matches!(c.as_str(), "text" | "json" | "tools"))
        {
            return Err(bad(
                "invalid proposal version, confidence, reason or capabilities",
            ));
        }
        if p.suggested_acceptance.version != acceptance.version {
            return Err(bad("planner cannot change the host acceptance version"));
        }
        acceptance.nonempty |= p.suggested_acceptance.nonempty;
        acceptance.json_object |= p.suggested_acceptance.json_object;
        for text in &p.suggested_acceptance.required_substrings {
            if !acceptance.required_substrings.contains(text) {
                acceptance.required_substrings.push(text.clone());
            }
        }
        constraints
            .required_capabilities
            .extend(p.required_capabilities.clone());
        PlanChoice {
            task_type: p.task_type,
            strategy: p.strategy.clone(),
        }
    } else if let Some(choice) = fallback {
        choice.clone()
    } else {
        PlanChoice {
            task_type: submission.task_type.unwrap_or_default(),
            strategy: submission
                .strategy
                .clone()
                .ok_or_else(|| bad("explicit strategy required without a proposal"))?,
        }
    };
    if submission.task_type.is_some_and(|t| t != choice.task_type)
        || submission
            .strategy
            .as_ref()
            .is_some_and(|s| *s != choice.strategy)
    {
        return Err(bad(
            "plan conflicts with explicit host task type or strategy",
        ));
    }
    if constraints.required_capabilities.contains("tools") && submission.tools.is_empty() {
        return Err(bad("tools capability requires host-declared tools"));
    }
    let role = if choice.strategy == Strategy::GeneratorCritic {
        "generator"
    } else {
        "invoke"
    };
    let mut role_profiles = BTreeMap::from([(
        role.into(),
        RoleProfile {
            task_type: choice.task_type,
            role: role.into(),
        },
    )]);
    if choice.strategy == Strategy::GeneratorCritic {
        role_profiles.insert(
            "critic".into(),
            RoleProfile {
                task_type: if choice.task_type == TaskType::CodeGeneration {
                    TaskType::CodeReview
                } else {
                    choice.task_type
                },
                role: "critic".into(),
            },
        );
    }
    Ok(EffectivePlan {
        plan_version: 1,
        task_type: choice.task_type,
        strategy: choice.strategy,
        role_profiles,
        constraints,
        acceptance,
        submission_hash: digest(submission),
        proposal_id: proposal.map(|(id, _)| id.into()),
        validator_version: "1".into(),
        routing_profile_version: "legacy-global-v1".into(),
    })
}
