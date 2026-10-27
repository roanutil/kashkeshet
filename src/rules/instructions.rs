//! Immutable instructions saved by the human operator.
use super::*;
use crate::store::OperatorInstructionsRecord;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperatorInstructions {
    pub version: i64,
    pub text: String,
    pub saved_at: String,
    pub run_id: i64,
}
impl From<OperatorInstructionsRecord> for OperatorInstructions {
    fn from(v: OperatorInstructionsRecord) -> Self {
        Self {
            version: v.version,
            text: v.text,
            saved_at: v.saved_at,
            run_id: v.run_id,
        }
    }
}
impl Rules {
    pub async fn current_operator_instructions(&self) -> Result<Option<OperatorInstructions>> {
        self.db()
            .read(|s| {
                Ok(s.current_operator_instructions()?
                    .map(OperatorInstructions::from))
            })
            .await
    }
    pub async fn operator_instructions_history(&self) -> Result<Vec<OperatorInstructions>> {
        self.db()
            .read(|s| {
                Ok(s.operator_instructions_history()?
                    .into_iter()
                    .map(OperatorInstructions::from)
                    .collect())
            })
            .await
    }
    /// Empty text is a new version which clears the current instructions.
    pub async fn save_operator_instructions(
        &self,
        actor: Actor,
        text: String,
    ) -> Result<OperatorInstructions> {
        if !matches!(actor, Actor::Human) {
            return Err(RuleError::invalid(
                "only the human operator may save operator instructions",
            ));
        }
        if text.chars().count() > self.limits().max_operator_instructions_chars {
            return Err(RuleError::invalid(format!(
                "operator instructions must be at most {} characters",
                self.limits().max_operator_instructions_chars
            )));
        }
        let at = self.now_text();
        let run_id = self.inner.deps.run_id;
        self.db()
            .write(move |s| {
                Ok(OperatorInstructions::from(
                    s.append_operator_instructions(&text, &at, run_id)?,
                ))
            })
            .await
    }
}
