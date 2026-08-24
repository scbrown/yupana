//! Turn-grounding projection for Bash action trace records.

use crate::{action, turn_grounding};

pub(super) fn action_fields<'a>(
    action: &'a action::Action,
    grounding: Option<(
        &'a turn_grounding::GroundingRef,
        turn_grounding::GroundingState,
    )>,
) -> Vec<(&'a str, serde_json::Value)> {
    let mut fields: Vec<(&str, serde_json::Value)> =
        vec![("target_class", action.target_class.as_str().into())];
    if let Some(verb) = &action.verb {
        fields.push(("verb", verb.clone().into()));
    }
    if let Some(target) = &action.target {
        fields.push(("target", target.clone().into()));
    }
    if let Some((reference, state)) = grounding {
        let outcome = match &state {
            turn_grounding::GroundingState::Used => crate::trace::Outcome::Satisfied,
            turn_grounding::GroundingState::Empty => crate::trace::Outcome::Unsatisfied,
            _ => crate::trace::Outcome::Unknown,
        };
        let response = if state.advice().is_some() {
            crate::trace::Response::Warned
        } else {
            crate::trace::Response::NoAction
        };
        let evaluation =
            crate::trace::ConstraintEvaluation::new("na-turn-grounding", outcome, response)
                .placed(
                    Some(crate::constraint::ConstraintClass::Soft),
                    Some(crate::constraint::VerificationPoint::Pag),
                )
                .hosted_at(crate::hosting::YUPANA_HOSTS_AT)
                .grounded(reference, &state);
        fields.push(("constraints", crate::trace::to_json(&[evaluation])));
        fields.push(("grounding_outcome", state.as_str().into()));
        if let Some(id) = &reference.grounding_id {
            fields.push(("grounding_id", id.clone().into()));
        }
        if let Some(faction) = &reference.faction_id {
            fields.push(("faction_id", faction.clone().into()));
        }
        if let Some(worldview) = &reference.worldview_sha256 {
            fields.push(("worldview_sha256", worldview.clone().into()));
        }
    }
    fields
}
