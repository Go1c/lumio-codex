use crate::{HarnessError, Result};
use serde::{Deserialize, Serialize};

pub const EFFECT_RECEIPT_SCHEMA: &str = "test-sync-effect/1";
pub const EFFECT_OBSERVATION_SCHEMA: &str = "test-sync-effect-observation/1";
pub const MAX_EFFECT_RECEIPT_BYTES: u64 = 16 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectAction {
    Reconnect,
    AgentRestart,
    AppRestart,
}

impl EffectAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reconnect => "reconnect",
            Self::AgentRestart => "agent_restart",
            Self::AppRestart => "app_restart",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectContext {
    pub workspace_id: String,
    pub client_id_a: String,
    pub client_id_b: String,
    pub agent_pid_a: i32,
    pub agent_pid_b: i32,
}

impl EffectContext {
    pub fn validate(&self) -> Result<()> {
        if self.workspace_id.is_empty()
            || self.client_id_a.is_empty()
            || self.client_id_b.is_empty()
            || self.client_id_a == self.client_id_b
            || self.agent_pid_a <= 0
            || self.agent_pid_b <= 0
            || self.agent_pid_a == self.agent_pid_b
        {
            return Err(HarnessError::InvalidConfiguration(
                "effect context is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectIdentity {
    pub pid: Option<i32>,
    pub generation: Option<u64>,
}

impl EffectIdentity {
    fn validate(self) -> Result<()> {
        if self.pid.is_none_or(|pid| pid <= 0) || self.generation.is_none() {
            return Err(HarnessError::InvalidConfiguration(
                "effect identity must contain a positive PID and generation",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectObservation {
    pub schema_version: String,
    pub action: EffectAction,
    pub context: EffectContext,
    pub identity: EffectIdentity,
}

impl EffectObservation {
    pub fn new(action: EffectAction, context: EffectContext, identity: EffectIdentity) -> Self {
        Self {
            schema_version: EFFECT_OBSERVATION_SCHEMA.to_owned(),
            action,
            context,
            identity,
        }
    }

    pub fn parse_and_validate(
        bytes: &[u8],
        expected_action: EffectAction,
        expected_context: &EffectContext,
    ) -> Result<Self> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_EFFECT_RECEIPT_BYTES {
            return Err(HarnessError::InvalidConfiguration(
                "effect observation size is invalid",
            ));
        }
        let observation: Self = serde_json::from_slice(bytes)?;
        observation.validate(expected_action, expected_context)?;
        Ok(observation)
    }

    pub fn validate(
        &self,
        expected_action: EffectAction,
        expected_context: &EffectContext,
    ) -> Result<()> {
        expected_context.validate()?;
        if self.schema_version != EFFECT_OBSERVATION_SCHEMA {
            return Err(HarnessError::InvalidConfiguration(
                "effect observation schema is invalid",
            ));
        }
        if self.action != expected_action || self.context != *expected_context {
            return Err(HarnessError::InvalidConfiguration(
                "effect observation does not match the requested context",
            ));
        }
        self.identity.validate()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectReceipt {
    pub schema_version: String,
    pub action: EffectAction,
    pub context: EffectContext,
    pub old: EffectIdentity,
    pub new: EffectIdentity,
}

impl EffectReceipt {
    pub fn observed_transition(
        action: EffectAction,
        context: EffectContext,
        old: EffectIdentity,
        new: EffectIdentity,
    ) -> Self {
        Self {
            schema_version: EFFECT_RECEIPT_SCHEMA.to_owned(),
            action,
            context,
            old,
            new,
        }
    }

    pub fn parse_and_validate(
        bytes: &[u8],
        expected_action: EffectAction,
        expected_context: &EffectContext,
    ) -> Result<Self> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_EFFECT_RECEIPT_BYTES {
            return Err(HarnessError::InvalidConfiguration(
                "effect receipt size is invalid",
            ));
        }
        let receipt: Self = serde_json::from_slice(bytes)?;
        receipt.validate_shape(expected_action, expected_context)?;
        Ok(receipt)
    }

    pub fn validate_observed(
        &self,
        expected_action: EffectAction,
        expected_context: &EffectContext,
        before: &EffectObservation,
        after: &EffectObservation,
    ) -> Result<()> {
        self.validate_shape(expected_action, expected_context)?;
        before.validate(expected_action, expected_context)?;
        after.validate(expected_action, expected_context)?;
        if self.old != before.identity || self.new != after.identity {
            return Err(HarnessError::InvalidConfiguration(
                "effect receipt does not match independent observations",
            ));
        }

        let old_pid = before.identity.pid.expect("validated effect PID");
        let new_pid = after.identity.pid.expect("validated effect PID");
        let old_generation = before
            .identity
            .generation
            .expect("validated effect generation");
        let new_generation = after
            .identity
            .generation
            .expect("validated effect generation");
        if new_generation <= old_generation {
            return Err(HarnessError::InvalidConfiguration(
                "effect generation did not advance",
            ));
        }
        let pid_transition_is_valid = match expected_action {
            EffectAction::Reconnect => old_pid == new_pid,
            EffectAction::AgentRestart | EffectAction::AppRestart => old_pid != new_pid,
        };
        if !pid_transition_is_valid {
            return Err(HarnessError::InvalidConfiguration(
                "effect PID transition does not match the requested action",
            ));
        }
        Ok(())
    }

    fn validate_shape(
        &self,
        expected_action: EffectAction,
        expected_context: &EffectContext,
    ) -> Result<()> {
        expected_context.validate()?;
        if self.schema_version != EFFECT_RECEIPT_SCHEMA {
            return Err(HarnessError::InvalidConfiguration(
                "effect receipt schema is invalid",
            ));
        }
        if self.action != expected_action || self.context != *expected_context {
            return Err(HarnessError::InvalidConfiguration(
                "effect receipt does not match the request",
            ));
        }
        self.old.validate()?;
        self.new.validate()?;
        if self.old == self.new {
            return Err(HarnessError::InvalidConfiguration(
                "effect receipt reports a no-op",
            ));
        }
        Ok(())
    }
}
