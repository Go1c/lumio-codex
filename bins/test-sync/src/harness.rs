use crate::agent::OwnedAgent;
use crate::cli::{RunArgs, Timeouts};
use crate::effect::{
    EffectAction, EffectContext, EffectIdentity, EffectObservation, EffectReceipt,
    MAX_EFFECT_RECEIPT_BYTES,
};
use crate::evidence::EvidenceWriter;
use crate::process::{
    CleanupFailure, OwnedChild, PinnedExecutable, ProcessOutcome, ProcessSpec, Termination,
};
use crate::scenario::{
    apply_action, deterministic_plan, write_conflict_side, CheckpointExpectation, Endpoint,
    ScenarioAction,
};
use crate::secret::SecretMaterial;
use crate::snapshot::{capture, CheckpointExpectationView, CheckpointSample, SnapshotExpectation};
use crate::stability::{classify_stability, Stability};
use crate::{io_error, HarnessError, Result};
use serde::Serialize;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
struct EndpointConfig {
    label: &'static str,
    endpoint: String,
    client_id: String,
    root: PathBuf,
    state: PathBuf,
}

#[derive(Debug)]
struct HarnessConfig {
    workspace_id: String,
    agent_binary: PathBuf,
    reconnect_hook: PathBuf,
    app_restart_hook: PathBuf,
    effect_observer: PinnedExecutable,
    endpoint_a: EndpointConfig,
    endpoint_b: EndpointConfig,
    timeouts: Timeouts,
    max_active_transfers: usize,
}

#[derive(Debug, Serialize)]
struct ProcessEvent<'a> {
    sequence: u64,
    component: &'a str,
    event: &'a str,
    pid: Option<i32>,
    pgid: Option<i32>,
    termination: Option<&'a str>,
    group_termination: Option<&'a str>,
    exit_code: Option<i32>,
    exit_signal: Option<i32>,
    term_attempted: Option<bool>,
    kill_attempted: Option<bool>,
    descendants_present: Option<bool>,
    leader_reaped: Option<bool>,
    group_empty: Option<bool>,
    cleanup_timed_out: Option<bool>,
    reason: Option<&'a str>,
    error: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct ObservationOutputEvent<'a> {
    sequence: u64,
    component: &'a str,
    event: &'static str,
    pid: i32,
    pgid: i32,
    stdout_bytes: usize,
    stdout_limit: u64,
    error: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct ProtocolEvent<'a> {
    sequence: u64,
    event: &'a str,
    checkpoint: Option<&'a str>,
    identical_samples: Option<usize>,
    manifest_a: Option<&'a str>,
    manifest_b: Option<&'a str>,
    ack_a: Option<&'a str>,
    ack_b: Option<&'a str>,
    conflicts: Option<u64>,
}

#[derive(Debug, Serialize)]
struct RunEvidence {
    status: &'static str,
    run_error: Option<String>,
    cleanup_a_error: Option<String>,
    cleanup_b_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct CheckpointFailureEvidence<'a> {
    checkpoint: &'a str,
    reason: &'a str,
    error: Option<&'a str>,
    last_sample: Option<&'a CheckpointSample>,
}

pub async fn run(args: RunArgs, cancellation: CancellationToken) -> Result<PathBuf> {
    let config = validate_config(&args)?;
    let secret = SecretMaterial::read(args.token_source()?)?;
    let plan = deterministic_plan(args.large_file_bytes)?;
    let evidence = match args.evidence_root.as_deref() {
        Some(root) => EvidenceWriter::create_in(root, &args.run_id, secret.redaction_bytes())?,
        None => EvidenceWriter::create(&args.run_id, secret.redaction_bytes())?,
    };
    evidence.write_json("scenario.json", &plan)?;

    let mut sequence = 0_u64;
    let mut agent_a: Option<OwnedAgent> = None;
    let mut agent_b: Option<OwnedAgent> = None;
    let run_result = run_scenario(
        &config,
        &secret,
        &plan,
        &evidence,
        &cancellation,
        &mut agent_a,
        &mut agent_b,
        &mut sequence,
    )
    .await;

    let cleanup_a = shutdown_agent(
        &mut agent_a,
        "agent_a",
        &config,
        &evidence,
        &cancellation,
        &mut sequence,
    )
    .await;
    let cleanup_b = shutdown_agent(
        &mut agent_b,
        "agent_b",
        &config,
        &evidence,
        &cancellation,
        &mut sequence,
    )
    .await;
    let final_event = if run_result.is_ok() && cleanup_a.is_ok() && cleanup_b.is_ok() {
        "completed"
    } else {
        "failed"
    };
    sequence += 1;
    evidence.append_event(
        "protocol",
        &ProtocolEvent {
            sequence,
            event: final_event,
            checkpoint: None,
            identical_samples: None,
            manifest_a: None,
            manifest_b: None,
            ack_a: None,
            ack_b: None,
            conflicts: None,
        },
    )?;
    evidence.write_json(
        "result.json",
        &RunEvidence {
            status: final_event,
            run_error: run_result.as_ref().err().map(ToString::to_string),
            cleanup_a_error: cleanup_a.as_ref().err().map(ToString::to_string),
            cleanup_b_error: cleanup_b.as_ref().err().map(ToString::to_string),
        },
    )?;
    let sums = evidence.finalize()?;
    let evidence_root = sums
        .parent()
        .ok_or(HarnessError::InvalidConfiguration(
            "checksum path has no parent",
        ))?
        .to_path_buf();
    run_result?;
    cleanup_a?;
    cleanup_b?;
    Ok(evidence_root)
}

#[allow(clippy::too_many_arguments)]
async fn run_scenario(
    config: &HarnessConfig,
    secret: &SecretMaterial,
    plan: &[ScenarioAction],
    evidence: &EvidenceWriter,
    cancellation: &CancellationToken,
    agent_a: &mut Option<OwnedAgent>,
    agent_b: &mut Option<OwnedAgent>,
    sequence: &mut u64,
) -> Result<()> {
    *agent_a = Some(
        spawn_agent(
            &config.endpoint_a,
            config,
            secret,
            evidence,
            cancellation,
            sequence,
        )
        .await?,
    );
    *agent_b = Some(
        spawn_agent(
            &config.endpoint_b,
            config,
            secret,
            evidence,
            cancellation,
            sequence,
        )
        .await?,
    );

    let mut checkpoint_index = 0_usize;
    let mut agent_generations = (1_u64, 1_u64);
    for action in plan {
        if cancellation.is_cancelled() {
            return Err(HarnessError::Process("harness cancelled"));
        }
        match action {
            ScenarioAction::ConcurrentConflict { path } => {
                concurrent_conflict(&config.endpoint_a.root, &config.endpoint_b.root, path).await?;
            }
            ScenarioAction::ReconnectHook => {
                let pids = running_agent_pids(agent_a, agent_b)?;
                run_observed_hook(
                    "reconnect_hook",
                    EffectAction::Reconnect,
                    &config.reconnect_hook,
                    config,
                    pids,
                    config.timeouts,
                    evidence,
                    cancellation,
                    sequence,
                )
                .await?;
            }
            ScenarioAction::RestartAgent { endpoint } => match endpoint {
                Endpoint::A => {
                    let old_pid = agent_a
                        .as_ref()
                        .ok_or(HarnessError::Process("agent A is not running"))?
                        .pid()
                        .as_raw_pid();
                    let context = effect_context(config, running_agent_pids(agent_a, agent_b)?)?;
                    let old_generation = agent_generations.0;
                    shutdown_agent(agent_a, "agent_a", config, evidence, cancellation, sequence)
                        .await?;
                    *agent_a = Some(
                        spawn_agent(
                            &config.endpoint_a,
                            config,
                            secret,
                            evidence,
                            cancellation,
                            sequence,
                        )
                        .await?,
                    );
                    let new_pid = agent_a
                        .as_ref()
                        .expect("agent A restarted")
                        .pid()
                        .as_raw_pid();
                    agent_generations.0 += 1;
                    record_internal_effect(
                        evidence,
                        EffectAction::AgentRestart,
                        context,
                        EffectIdentity {
                            pid: Some(old_pid),
                            generation: Some(old_generation),
                        },
                        EffectIdentity {
                            pid: Some(new_pid),
                            generation: Some(agent_generations.0),
                        },
                        sequence,
                    )?;
                }
                Endpoint::B => {
                    let old_pid = agent_b
                        .as_ref()
                        .ok_or(HarnessError::Process("agent B is not running"))?
                        .pid()
                        .as_raw_pid();
                    let context = effect_context(config, running_agent_pids(agent_a, agent_b)?)?;
                    let old_generation = agent_generations.1;
                    shutdown_agent(agent_b, "agent_b", config, evidence, cancellation, sequence)
                        .await?;
                    *agent_b = Some(
                        spawn_agent(
                            &config.endpoint_b,
                            config,
                            secret,
                            evidence,
                            cancellation,
                            sequence,
                        )
                        .await?,
                    );
                    let new_pid = agent_b
                        .as_ref()
                        .expect("agent B restarted")
                        .pid()
                        .as_raw_pid();
                    agent_generations.1 += 1;
                    record_internal_effect(
                        evidence,
                        EffectAction::AgentRestart,
                        context,
                        EffectIdentity {
                            pid: Some(old_pid),
                            generation: Some(old_generation),
                        },
                        EffectIdentity {
                            pid: Some(new_pid),
                            generation: Some(agent_generations.1),
                        },
                        sequence,
                    )?;
                }
            },
            ScenarioAction::RestartAppHook => {
                let pids = running_agent_pids(agent_a, agent_b)?;
                run_observed_hook(
                    "app_restart_hook",
                    EffectAction::AppRestart,
                    &config.app_restart_hook,
                    config,
                    pids,
                    config.timeouts,
                    evidence,
                    cancellation,
                    sequence,
                )
                .await?;
            }
            ScenarioAction::Checkpoint { name, expectation } => {
                let pids = (
                    agent_a
                        .as_ref()
                        .ok_or(HarnessError::Process("agent A is not running"))?
                        .pid()
                        .as_raw_pid(),
                    agent_b
                        .as_ref()
                        .ok_or(HarnessError::Process("agent B is not running"))?
                        .pid()
                        .as_raw_pid(),
                );
                stable_checkpoint(
                    checkpoint_index,
                    name,
                    expectation,
                    pids,
                    config,
                    evidence,
                    cancellation,
                    sequence,
                )
                .await?;
                checkpoint_index += 1;
            }
            _ => apply_action(&config.endpoint_a.root, &config.endpoint_b.root, action)?,
        }
    }
    Ok(())
}

async fn spawn_agent(
    endpoint: &EndpointConfig,
    config: &HarnessConfig,
    secret: &SecretMaterial,
    evidence: &EvidenceWriter,
    cancellation: &CancellationToken,
    sequence: &mut u64,
) -> Result<OwnedAgent> {
    let agent_config = fns_agent::AgentConfig {
        schema_version: "fns-agent-config/1".into(),
        endpoint: endpoint.endpoint.clone(),
        workspace_id: fns_protocol::WorkspaceId::parse(&config.workspace_id)
            .map_err(|_| HarnessError::InvalidConfiguration("workspace ID is invalid"))?,
        client_id: fns_protocol::ClientId::parse(&endpoint.client_id)
            .map_err(|_| HarnessError::InvalidConfiguration("client ID is invalid"))?,
        workspace_root: endpoint.root.clone(),
        state_dir: endpoint.state.clone(),
        token_file: endpoint.state.join("ipc-token-not-on-disk"),
        sync: fns_agent::config::AgentSyncConfig {
            includes: vec!["**".into()],
            excludes: vec![],
            protect_secrets: true,
        },
        transport: fns_agent::config::AgentTransportConfig {
            max_active_transfers: config.max_active_transfers,
        },
    };
    let mut agent = match OwnedAgent::launch(endpoint.label, &config.agent_binary) {
        Ok(agent) => agent,
        Err(error) => {
            record_process_failure(
                evidence,
                endpoint.label,
                "spawn_failed",
                None,
                None,
                &error.to_string(),
                sequence,
            )?;
            return Err(error);
        }
    };
    *sequence += 1;
    evidence.append_event(
        "process",
        &ProcessEvent {
            sequence: *sequence,
            component: endpoint.label,
            event: "spawned",
            pid: Some(agent.pid().as_raw_pid()),
            pgid: Some(agent.pgid().as_raw_pid()),
            termination: None,
            group_termination: None,
            exit_code: None,
            exit_signal: None,
            term_attempted: None,
            kill_attempted: None,
            descendants_present: None,
            leader_reaped: None,
            group_empty: None,
            cleanup_timed_out: None,
            reason: None,
            error: None,
        },
    )?;
    if let Err(error) = agent
        .bootstrap(agent_config, secret, config.timeouts, cancellation)
        .await
    {
        let pid = agent.pid().as_raw_pid();
        let pgid = agent.pgid().as_raw_pid();
        match agent.force_cleanup(config.timeouts).await {
            Ok(outcome) => record_process_outcome(
                evidence,
                endpoint.label,
                "reaped_after_startup_failure",
                outcome,
                sequence,
            )?,
            Err(cleanup_error) => record_cleanup_error(
                evidence,
                endpoint.label,
                "startup_cleanup_failed",
                &error.to_string(),
                &cleanup_error,
                pid,
                pgid,
                sequence,
            )?,
        }
        return Err(error);
    }
    Ok(agent)
}

async fn shutdown_agent(
    agent: &mut Option<OwnedAgent>,
    label: &'static str,
    config: &HarnessConfig,
    evidence: &EvidenceWriter,
    cancellation: &CancellationToken,
    sequence: &mut u64,
) -> Result<()> {
    let Some(mut owned) = agent.take() else {
        return Ok(());
    };
    match owned.shutdown(config.timeouts, cancellation).await {
        Ok(outcome) => record_process_outcome(evidence, label, "reaped", outcome, sequence),
        Err(error) => {
            let pid = owned.pid().as_raw_pid();
            let pgid = owned.pgid().as_raw_pid();
            if matches!(&error, HarnessError::Cleanup(_)) {
                record_cleanup_error(
                    evidence,
                    label,
                    "shutdown_cleanup_failed",
                    "agent shutdown",
                    &error,
                    pid,
                    pgid,
                    sequence,
                )?;
            }
            match owned.force_cleanup(config.timeouts).await {
                Ok(outcome) => record_process_outcome(
                    evidence,
                    label,
                    "reaped_after_shutdown_failure",
                    outcome,
                    sequence,
                )?,
                Err(cleanup_error) => record_cleanup_error(
                    evidence,
                    label,
                    "shutdown_cleanup_failed",
                    &error.to_string(),
                    &cleanup_error,
                    pid,
                    pgid,
                    sequence,
                )?,
            }
            Err(error)
        }
    }
}

#[derive(Debug)]
struct HookResult {
    receipt: EffectReceipt,
    pid: i32,
    pgid: i32,
}

#[allow(clippy::too_many_arguments)]
async fn run_observed_hook(
    label: &'static str,
    action: EffectAction,
    program: &Path,
    config: &HarnessConfig,
    agent_pids: (i32, i32),
    timeouts: Timeouts,
    evidence: &EvidenceWriter,
    cancellation: &CancellationToken,
    sequence: &mut u64,
) -> Result<()> {
    let context = effect_context(config, agent_pids)?;
    let before = observe_effect(
        action,
        "before",
        &context,
        config,
        timeouts,
        evidence,
        cancellation,
        sequence,
    )
    .await?;
    let result = run_hook(
        label,
        action,
        program,
        config,
        agent_pids,
        &context,
        timeouts,
        evidence,
        cancellation,
        sequence,
    )
    .await?;
    let after = observe_effect(
        action,
        "after",
        &context,
        config,
        timeouts,
        evidence,
        cancellation,
        sequence,
    )
    .await?;
    if let Err(error) = result
        .receipt
        .validate_observed(action, &context, &before, &after)
    {
        record_process_failure(
            evidence,
            label,
            "effect_validation_failed",
            Some(result.pid),
            Some(result.pgid),
            &error.to_string(),
            sequence,
        )?;
        return Err(error);
    }
    record_effect(evidence, &result.receipt, sequence)
}

#[allow(clippy::too_many_arguments)]
async fn run_hook(
    label: &'static str,
    action: EffectAction,
    program: &Path,
    config: &HarnessConfig,
    agent_pids: (i32, i32),
    context: &EffectContext,
    timeouts: Timeouts,
    evidence: &EvidenceWriter,
    cancellation: &CancellationToken,
    sequence: &mut u64,
) -> Result<HookResult> {
    let arguments = [
        "--action".to_owned(),
        action.as_str().to_owned(),
        "--workspace-id".to_owned(),
        config.workspace_id.clone(),
        "--client-id-a".to_owned(),
        config.endpoint_a.client_id.clone(),
        "--client-id-b".to_owned(),
        config.endpoint_b.client_id.clone(),
        "--agent-pid-a".to_owned(),
        agent_pids.0.to_string(),
        "--agent-pid-b".to_owned(),
        agent_pids.1.to_string(),
    ];
    let mut child = match OwnedChild::spawn(ProcessSpec::output(label, program, arguments)) {
        Ok(child) => child,
        Err(error) => {
            record_process_failure(
                evidence,
                label,
                "spawn_failed",
                None,
                None,
                &error.to_string(),
                sequence,
            )?;
            return Err(error);
        }
    };
    let stdout = child
        .take_stdout()
        .ok_or(HarnessError::Process("hook stdout pipe was unavailable"))?;
    *sequence += 1;
    evidence.append_event(
        "process",
        &ProcessEvent {
            sequence: *sequence,
            component: label,
            event: "spawned",
            pid: Some(child.pid().as_raw_pid()),
            pgid: Some(child.pgid().as_raw_pid()),
            termination: None,
            group_termination: None,
            exit_code: None,
            exit_signal: None,
            term_attempted: None,
            kill_attempted: None,
            descendants_present: None,
            leader_reaped: None,
            group_empty: None,
            cleanup_timed_out: None,
            reason: None,
            error: None,
        },
    )?;
    let operation = async {
        let mut receipt = Vec::new();
        let mut limited_stdout = stdout.take(MAX_EFFECT_RECEIPT_BYTES + 1);
        tokio::select! {
            result = limited_stdout.read_to_end(&mut receipt) => {
                result.map_err(|error| io_error(label, error))?;
            }
            () = cancellation.cancelled() => {
                return Err(HarnessError::Process("effect hook cancelled"));
            }
        }
        if receipt.len() as u64 > MAX_EFFECT_RECEIPT_BYTES {
            return Err(HarnessError::InvalidConfiguration(
                "effect receipt size is invalid",
            ));
        }
        let outcome = child
            .wait_or_cancel(cancellation, timeouts.term_grace, timeouts.kill)
            .await?;
        Ok((outcome, receipt))
    };
    let (outcome, receipt_bytes) = match tokio::time::timeout(timeouts.hook, operation).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            let pid = child.pid().as_raw_pid();
            let pgid = child.pgid().as_raw_pid();
            if matches!(&error, HarnessError::Cleanup(_)) {
                record_cleanup_error(
                    evidence,
                    label,
                    "hook_cleanup_failed",
                    "hook completion",
                    &error,
                    pid,
                    pgid,
                    sequence,
                )?;
            }
            let cleanup = child
                .terminate_and_reap(timeouts.term_grace, timeouts.kill)
                .await;
            record_process_failure(
                evidence,
                label,
                "hook_failed",
                Some(pid),
                Some(pgid),
                &error.to_string(),
                sequence,
            )?;
            match cleanup {
                Ok(outcome) => record_process_outcome(
                    evidence,
                    label,
                    "reaped_after_error",
                    outcome,
                    sequence,
                )?,
                Err(cleanup_error) => record_cleanup_error(
                    evidence,
                    label,
                    "hook_cleanup_failed",
                    &error.to_string(),
                    &cleanup_error,
                    pid,
                    pgid,
                    sequence,
                )?,
            }
            return Err(error);
        }
        Err(_) => {
            let pid = child.pid().as_raw_pid();
            let pgid = child.pgid().as_raw_pid();
            let error = HarnessError::Timeout("effect hook");
            let cleanup = child
                .terminate_and_reap(timeouts.term_grace, timeouts.kill)
                .await;
            record_process_failure(
                evidence,
                label,
                "hook_timed_out",
                Some(pid),
                Some(pgid),
                &error.to_string(),
                sequence,
            )?;
            match cleanup {
                Ok(outcome) => record_process_outcome(
                    evidence,
                    label,
                    "reaped_after_timeout",
                    outcome,
                    sequence,
                )?,
                Err(cleanup_error) => record_cleanup_error(
                    evidence,
                    label,
                    "hook_cleanup_failed",
                    &error.to_string(),
                    &cleanup_error,
                    pid,
                    pgid,
                    sequence,
                )?,
            }
            return Err(error);
        }
    };
    let success = outcome.status.success();
    record_process_outcome(evidence, label, "reaped", outcome, sequence)?;
    if !success {
        let error = HarnessError::Process("restart hook exited unsuccessfully");
        record_process_failure(
            evidence,
            label,
            "hook_failed",
            Some(outcome.pid.as_raw_pid()),
            Some(outcome.pgid.as_raw_pid()),
            &error.to_string(),
            sequence,
        )?;
        return Err(error);
    }
    if cancellation.is_cancelled() {
        let error = HarnessError::Process("effect hook cancelled");
        record_process_failure(
            evidence,
            label,
            "hook_failed",
            Some(outcome.pid.as_raw_pid()),
            Some(outcome.pgid.as_raw_pid()),
            &error.to_string(),
            sequence,
        )?;
        return Err(error);
    }
    let receipt = match EffectReceipt::parse_and_validate(&receipt_bytes, action, context) {
        Ok(receipt) => receipt,
        Err(error) => {
            record_process_failure(
                evidence,
                label,
                "effect_validation_failed",
                Some(outcome.pid.as_raw_pid()),
                Some(outcome.pgid.as_raw_pid()),
                &error.to_string(),
                sequence,
            )?;
            return Err(error);
        }
    };
    Ok(HookResult {
        receipt,
        pid: outcome.pid.as_raw_pid(),
        pgid: outcome.pgid.as_raw_pid(),
    })
}

fn running_agent_pids(
    agent_a: &Option<OwnedAgent>,
    agent_b: &Option<OwnedAgent>,
) -> Result<(i32, i32)> {
    Ok((
        agent_a
            .as_ref()
            .ok_or(HarnessError::Process("agent A is not running"))?
            .pid()
            .as_raw_pid(),
        agent_b
            .as_ref()
            .ok_or(HarnessError::Process("agent B is not running"))?
            .pid()
            .as_raw_pid(),
    ))
}

fn effect_context(config: &HarnessConfig, agent_pids: (i32, i32)) -> Result<EffectContext> {
    let context = EffectContext {
        workspace_id: config.workspace_id.clone(),
        client_id_a: config.endpoint_a.client_id.clone(),
        client_id_b: config.endpoint_b.client_id.clone(),
        agent_pid_a: agent_pids.0,
        agent_pid_b: agent_pids.1,
    };
    context.validate()?;
    Ok(context)
}

#[allow(clippy::too_many_arguments)]
async fn observe_effect(
    action: EffectAction,
    phase: &'static str,
    context: &EffectContext,
    config: &HarnessConfig,
    timeouts: Timeouts,
    evidence: &EvidenceWriter,
    cancellation: &CancellationToken,
    sequence: &mut u64,
) -> Result<EffectObservation> {
    let label = match phase {
        "before" => "effect_observer_before",
        "after" => "effect_observer_after",
        _ => {
            return Err(HarnessError::InvalidConfiguration(
                "effect observation phase is invalid",
            ))
        }
    };
    let arguments = [
        "--action".to_owned(),
        action.as_str().to_owned(),
        "--phase".to_owned(),
        phase.to_owned(),
        "--workspace-id".to_owned(),
        context.workspace_id.clone(),
        "--client-id-a".to_owned(),
        context.client_id_a.clone(),
        "--client-id-b".to_owned(),
        context.client_id_b.clone(),
        "--agent-pid-a".to_owned(),
        context.agent_pid_a.to_string(),
        "--agent-pid-b".to_owned(),
        context.agent_pid_b.to_string(),
    ];
    let mut child = match OwnedChild::spawn_pinned(
        ProcessSpec::output(label, Path::new("pinned-effect-observer"), arguments),
        &config.effect_observer,
    ) {
        Ok(child) => child,
        Err(error) => {
            record_process_failure(
                evidence,
                label,
                "spawn_failed",
                None,
                None,
                &error.to_string(),
                sequence,
            )?;
            return Err(error);
        }
    };
    let stdout = child.take_stdout().ok_or(HarnessError::Process(
        "effect observer stdout pipe was unavailable",
    ))?;
    let pid = child.pid().as_raw_pid();
    let pgid = child.pgid().as_raw_pid();
    *sequence += 1;
    evidence.append_event(
        "process",
        &ProcessEvent {
            sequence: *sequence,
            component: label,
            event: "spawned",
            pid: Some(pid),
            pgid: Some(pgid),
            termination: None,
            group_termination: None,
            exit_code: None,
            exit_signal: None,
            term_attempted: None,
            kill_attempted: None,
            descendants_present: None,
            leader_reaped: None,
            group_empty: None,
            cleanup_timed_out: None,
            reason: None,
            error: None,
        },
    )?;

    let operation = async {
        let mut bytes = Vec::new();
        let mut limited = stdout.take(MAX_EFFECT_RECEIPT_BYTES + 1);
        tokio::select! {
            result = limited.read_to_end(&mut bytes) => {
                result.map_err(|error| io_error(label, error))?;
            }
            () = cancellation.cancelled() => {
                return Err(HarnessError::Process("effect observation cancelled"));
            }
        }
        if bytes.len() as u64 > MAX_EFFECT_RECEIPT_BYTES {
            return Err(HarnessError::InvalidConfiguration(
                "effect observation size is invalid",
            ));
        }
        let outcome = child
            .wait_or_cancel(cancellation, timeouts.term_grace, timeouts.kill)
            .await?;
        Ok((outcome, bytes))
    };
    let (outcome, bytes) = match tokio::time::timeout(timeouts.hook, operation).await {
        Ok(Ok(result)) => result,
        result => {
            let error = match result {
                Ok(Err(error)) => error,
                Err(_) => HarnessError::Timeout("effect observation"),
                Ok(Ok(_)) => unreachable!(),
            };
            if matches!(&error, HarnessError::Cleanup(_)) {
                record_cleanup_error(
                    evidence,
                    label,
                    "observation_cleanup_failed",
                    "effect observation completion",
                    &error,
                    pid,
                    pgid,
                    sequence,
                )?;
            }
            record_process_failure(
                evidence,
                label,
                "observation_failed",
                Some(pid),
                Some(pgid),
                &error.to_string(),
                sequence,
            )?;
            match child
                .terminate_and_reap(timeouts.term_grace, timeouts.kill)
                .await
            {
                Ok(outcome) => record_process_outcome(
                    evidence,
                    label,
                    "reaped_after_observation_failure",
                    outcome,
                    sequence,
                )?,
                Err(cleanup_error) => record_cleanup_error(
                    evidence,
                    label,
                    "observation_cleanup_failed",
                    &error.to_string(),
                    &cleanup_error,
                    pid,
                    pgid,
                    sequence,
                )?,
            }
            return Err(error);
        }
    };
    let success = outcome.status.success();
    record_process_outcome(evidence, label, "reaped", outcome, sequence)?;
    if !success {
        let error = HarnessError::Process("effect observer exited unsuccessfully");
        record_process_failure(
            evidence,
            label,
            "observation_failed",
            Some(pid),
            Some(pgid),
            &error.to_string(),
            sequence,
        )?;
        return Err(error);
    }
    let observation = EffectObservation::parse_and_validate(&bytes, action, context);
    let error = observation.as_ref().err().map(ToString::to_string);
    *sequence += 1;
    evidence.append_event(
        "process",
        &ObservationOutputEvent {
            sequence: *sequence,
            component: label,
            event: if error.is_some() {
                "observation_output_invalid"
            } else {
                "observation_output_validated"
            },
            pid,
            pgid,
            stdout_bytes: bytes.len(),
            stdout_limit: MAX_EFFECT_RECEIPT_BYTES,
            error: error.as_deref(),
        },
    )?;
    observation
}

fn record_internal_effect(
    evidence: &EvidenceWriter,
    action: EffectAction,
    context: EffectContext,
    old: EffectIdentity,
    new: EffectIdentity,
    sequence: &mut u64,
) -> Result<()> {
    let before = EffectObservation::new(action, context.clone(), old);
    let after = EffectObservation::new(action, context.clone(), new);
    let receipt = EffectReceipt::observed_transition(action, context.clone(), old, new);
    receipt.validate_observed(action, &context, &before, &after)?;
    record_effect(evidence, &receipt, sequence)
}

fn record_effect(
    evidence: &EvidenceWriter,
    receipt: &EffectReceipt,
    sequence: &mut u64,
) -> Result<()> {
    *sequence += 1;
    evidence.write_json(
        &format!("effects/{:04}-{}.json", *sequence, receipt.action.as_str()),
        receipt,
    )
}

fn record_process_outcome(
    evidence: &EvidenceWriter,
    label: &'static str,
    event: &'static str,
    outcome: ProcessOutcome,
    sequence: &mut u64,
) -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;

    *sequence += 1;
    evidence.append_event(
        "process",
        &ProcessEvent {
            sequence: *sequence,
            component: label,
            event,
            pid: Some(outcome.pid.as_raw_pid()),
            pgid: Some(outcome.pgid.as_raw_pid()),
            termination: Some(termination_name(outcome.termination)),
            group_termination: Some(termination_name(outcome.group_cleanup.termination)),
            exit_code: outcome.status.code(),
            #[cfg(unix)]
            exit_signal: outcome.status.signal(),
            #[cfg(not(unix))]
            exit_signal: None,
            term_attempted: Some(outcome.group_cleanup.term_attempted),
            kill_attempted: Some(outcome.group_cleanup.kill_attempted),
            descendants_present: Some(outcome.group_cleanup.descendants_present),
            leader_reaped: Some(true),
            group_empty: Some(outcome.group_cleanup.group_empty),
            cleanup_timed_out: Some(false),
            reason: Some(event),
            error: None,
        },
    )
}

fn record_process_failure(
    evidence: &EvidenceWriter,
    label: &'static str,
    event: &'static str,
    pid: Option<i32>,
    pgid: Option<i32>,
    error: &str,
    sequence: &mut u64,
) -> Result<()> {
    *sequence += 1;
    evidence.append_event(
        "process",
        &ProcessEvent {
            sequence: *sequence,
            component: label,
            event,
            pid,
            pgid,
            termination: None,
            group_termination: None,
            exit_code: None,
            exit_signal: None,
            term_attempted: None,
            kill_attempted: None,
            descendants_present: None,
            leader_reaped: None,
            group_empty: None,
            cleanup_timed_out: None,
            reason: Some(event),
            error: Some(error),
        },
    )
}

fn record_cleanup_failure(
    evidence: &EvidenceWriter,
    label: &'static str,
    event: &'static str,
    reason: &str,
    failure: &CleanupFailure,
    sequence: &mut u64,
) -> Result<()> {
    *sequence += 1;
    evidence.append_event(
        "process",
        &ProcessEvent {
            sequence: *sequence,
            component: label,
            event,
            pid: Some(failure.pid.as_raw_pid()),
            pgid: Some(failure.pgid.as_raw_pid()),
            termination: failure.leader_termination.map(termination_name),
            group_termination: None,
            exit_code: failure.exit_code,
            exit_signal: failure.exit_signal,
            term_attempted: Some(failure.term_attempted),
            kill_attempted: Some(failure.kill_attempted),
            descendants_present: Some(failure.descendants_present),
            leader_reaped: Some(failure.leader_reaped),
            group_empty: Some(failure.group_empty),
            cleanup_timed_out: Some(failure.timed_out),
            reason: Some(reason),
            error: Some(failure.detail.as_str()),
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn record_cleanup_error(
    evidence: &EvidenceWriter,
    label: &'static str,
    event: &'static str,
    reason: &str,
    error: &HarnessError,
    pid: i32,
    pgid: i32,
    sequence: &mut u64,
) -> Result<()> {
    match error {
        HarnessError::Cleanup(failure) => {
            record_cleanup_failure(evidence, label, event, reason, failure, sequence)
        }
        _ => record_process_failure(
            evidence,
            label,
            event,
            Some(pid),
            Some(pgid),
            &error.to_string(),
            sequence,
        ),
    }
}

fn termination_name(termination: Termination) -> &'static str {
    match termination {
        Termination::Exited => "exit",
        Termination::Terminated => "term",
        Termination::Killed => "kill",
    }
}

async fn concurrent_conflict(root_a: &Path, root_b: &Path, path: &str) -> Result<()> {
    let root_a = root_a.to_path_buf();
    let root_b = root_b.to_path_buf();
    let path_a = path.to_owned();
    let path_b = path.to_owned();
    let barrier = Arc::new(Barrier::new(2));
    let barrier_a = Arc::clone(&barrier);
    let barrier_b = Arc::clone(&barrier);
    let a = tokio::task::spawn_blocking(move || {
        barrier_a.wait();
        write_conflict_side(&root_a, &path_a, Endpoint::A)
    });
    let b = tokio::task::spawn_blocking(move || {
        barrier_b.wait();
        write_conflict_side(&root_b, &path_b, Endpoint::B)
    });
    a.await
        .map_err(|_| HarnessError::Process("conflict task A panicked"))??;
    b.await
        .map_err(|_| HarnessError::Process("conflict task B panicked"))??;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn stable_checkpoint(
    index: usize,
    name: &str,
    expectation: &CheckpointExpectation,
    expected_pids: (i32, i32),
    config: &HarnessConfig,
    evidence: &EvidenceWriter,
    cancellation: &CancellationToken,
    sequence: &mut u64,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + config.timeouts.checkpoint;
    let mut samples = VecDeque::with_capacity(3);
    let mut stability_samples = VecDeque::with_capacity(3);
    let mut attempt = 0_u64;
    loop {
        if cancellation.is_cancelled() {
            persist_checkpoint_failure(evidence, index, name, "cancelled", None, samples.back())?;
            return Err(HarnessError::Process("checkpoint cancelled"));
        }
        let root_a = config.endpoint_a.root.clone();
        let root_b = config.endpoint_b.root.clone();
        let state_a = config.endpoint_a.state.clone();
        let state_b = config.endpoint_b.state.clone();
        let workspace_id = config.workspace_id.clone();
        let captured = tokio::task::spawn_blocking(move || {
            capture(&root_a, &root_b, &state_a, &state_b, &workspace_id)
        })
        .await;
        let sample = match captured {
            Ok(Ok(sample)) => sample,
            Ok(Err(error)) => {
                persist_checkpoint_failure(
                    evidence,
                    index,
                    name,
                    "capture_failed",
                    Some(&error.to_string()),
                    samples.back(),
                )?;
                return Err(error);
            }
            Err(_) => {
                let error = HarnessError::Process("checkpoint capture panicked");
                persist_checkpoint_failure(
                    evidence,
                    index,
                    name,
                    "capture_panicked",
                    Some(&error.to_string()),
                    samples.back(),
                )?;
                return Err(error);
            }
        };
        attempt += 1;
        evidence.write_json(
            &format!("checkpoints/{index:02}-{name}-sample-{attempt:04}.json"),
            &sample,
        )?;
        if samples.len() == 3 {
            samples.pop_front();
            stability_samples.pop_front();
        }
        stability_samples.push_back(sample.stability_projection());
        samples.push_back(sample);
        let sample_slice = stability_samples.make_contiguous();
        let expected = SnapshotExpectation {
            workspace_id: &config.workspace_id,
            client_id_a: &config.endpoint_a.client_id,
            client_id_b: &config.endpoint_b.client_id,
            pids: expected_pids,
        };
        let expectation_view = match expectation {
            CheckpointExpectation::Converged => CheckpointExpectationView::Converged,
            CheckpointExpectation::Conflict { path, kind } => {
                CheckpointExpectationView::Conflict { path, kind }
            }
        };
        let classification = classify_stability(sample_slice, |sample| match expectation {
            CheckpointExpectation::Converged => sample.converged(&expected),
            CheckpointExpectation::Conflict { path, kind } => {
                sample.conflict_stable(&expected, path, kind)
            }
        });
        let last = samples.back().expect("checkpoint sample exists");
        let identical = match classification {
            Stability::Collecting { identical } => identical,
            Stability::Rejected | Stability::Stable => 3,
        };
        let rejection_reasons = last.rejection_reasons(&expected, expectation_view);
        *sequence += 1;
        evidence.append_event(
            "protocol",
            &ProtocolEvent {
                sequence: *sequence,
                event: match classification {
                    Stability::Collecting { .. } => "checkpoint_collecting",
                    Stability::Rejected => "checkpoint_not_ready",
                    Stability::Stable => "checkpoint_stable",
                },
                checkpoint: Some(name),
                identical_samples: Some(identical),
                manifest_a: Some(&last.manifest_a.digest),
                manifest_b: Some(&last.manifest_b.digest),
                ack_a: last
                    .client_a
                    .cursor
                    .as_ref()
                    .map(|cursor| cursor.last_ack_revision.as_str()),
                ack_b: last
                    .client_b
                    .cursor
                    .as_ref()
                    .map(|cursor| cursor.last_ack_revision.as_str()),
                conflicts: Some(
                    u64::try_from(last.client_a.conflicts.len() + last.client_b.conflicts.len())
                        .unwrap_or(u64::MAX),
                ),
            },
        )?;
        if classification == Stability::Stable {
            evidence.write_json(&format!("checkpoints/{index:02}-{name}.json"), last)?;
            return Ok(());
        }
        // Fail fast: agent runtime is already terminal (auth rejected, etc.).
        if last.blocked_by_terminal_runtime() {
            let detail = format!(
                "checkpoint {name}: terminal agent runtime ({})",
                rejection_reasons.join(",")
            );
            persist_checkpoint_failure(
                evidence,
                index,
                name,
                "terminal_runtime",
                Some(&detail),
                samples.back(),
            )?;
            return Err(HarnessError::ProcessDetail(detail));
        }
        // Fail fast: Converged expectation cannot succeed while unresolved
        // conflicts remain and byte manifests already match (dirty remote WS).
        if matches!(expectation, CheckpointExpectation::Converged)
            && last.converged_blocked_by_stable_conflicts(&expected)
            && identical >= 3
        {
            let detail = format!(
                "checkpoint {name}: converged blocked by unresolved conflicts ({})",
                rejection_reasons.join(",")
            );
            persist_checkpoint_failure(
                evidence,
                index,
                name,
                "blocked_by_unresolved_conflicts",
                Some(&detail),
                samples.back(),
            )?;
            return Err(HarnessError::ProcessDetail(detail));
        }
        if tokio::time::Instant::now() >= deadline {
            let detail = if rejection_reasons.is_empty() {
                format!("checkpoint {name}: timed out waiting for three stable samples")
            } else {
                format!(
                    "checkpoint {name}: timed out ({})",
                    rejection_reasons.join(",")
                )
            };
            persist_checkpoint_failure(
                evidence,
                index,
                name,
                "timeout",
                Some(&detail),
                samples.back(),
            )?;
            return Err(HarnessError::ProcessDetail(detail));
        }
        tokio::select! {
            () = tokio::time::sleep(config.timeouts.sample_interval) => {}
            () = cancellation.cancelled() => {
                persist_checkpoint_failure(
                    evidence,
                    index,
                    name,
                    "cancelled",
                    None,
                    samples.back(),
                )?;
                return Err(HarnessError::Process("checkpoint cancelled"));
            },
        }
    }
}

fn persist_checkpoint_failure(
    evidence: &EvidenceWriter,
    index: usize,
    name: &str,
    reason: &str,
    error: Option<&str>,
    last_sample: Option<&CheckpointSample>,
) -> Result<()> {
    evidence.write_json(
        &format!("checkpoints/{index:02}-{name}-failure-{reason}.json"),
        &CheckpointFailureEvidence {
            checkpoint: name,
            reason,
            error,
            last_sample,
        },
    )
}

fn validate_config(args: &RunArgs) -> Result<HarnessConfig> {
    let timeouts = args.timeouts()?;
    if args.endpoint_a == args.endpoint_b {
        return Err(HarnessError::InvalidConfiguration(
            "two distinct real endpoints are required",
        ));
    }
    fns_transport::WorkspaceEndpoint::parse(&args.endpoint_a)
        .map_err(|_| HarnessError::InvalidConfiguration("endpoint A is invalid"))?;
    fns_transport::WorkspaceEndpoint::parse(&args.endpoint_b)
        .map_err(|_| HarnessError::InvalidConfiguration("endpoint B is invalid"))?;
    if args.client_id_a == args.client_id_b {
        return Err(HarnessError::InvalidConfiguration(
            "two distinct client IDs are required",
        ));
    }
    fns_protocol::WorkspaceId::parse(&args.workspace_id)
        .map_err(|_| HarnessError::InvalidConfiguration("workspace ID is invalid"))?;
    fns_protocol::ClientId::parse(&args.client_id_a)
        .map_err(|_| HarnessError::InvalidConfiguration("client ID A is invalid"))?;
    fns_protocol::ClientId::parse(&args.client_id_b)
        .map_err(|_| HarnessError::InvalidConfiguration("client ID B is invalid"))?;
    if args.max_active_transfers == 0
        || args.max_active_transfers > fns_transport::MAX_ACTIVE_TRANSFERS
    {
        return Err(HarnessError::InvalidConfiguration(
            "max active transfers must be positive",
        ));
    }
    let agent_binary = canonical_executable(&args.agent_binary)?;
    let reconnect_hook = canonical_executable(&args.reconnect_hook)?;
    let app_restart_hook = canonical_executable(&args.app_restart_hook)?;
    let effect_observer_path = canonical_executable(&args.effect_observer)?;
    let effect_observer = PinnedExecutable::pin(&effect_observer_path)?;
    ensure_independent_observer(
        &effect_observer,
        &[&agent_binary, &reconnect_hook, &app_restart_hook],
    )?;
    let root_a = prepare_private_directory(&args.root_a)?;
    let root_b = prepare_private_directory(&args.root_b)?;
    let state_a = prepare_private_directory(&args.state_a)?;
    let state_b = prepare_private_directory(&args.state_b)?;
    ensure_disjoint(&[&root_a, &root_b, &state_a, &state_b])?;
    Ok(HarnessConfig {
        workspace_id: args.workspace_id.clone(),
        agent_binary,
        reconnect_hook,
        app_restart_hook,
        effect_observer,
        endpoint_a: EndpointConfig {
            label: "agent_a",
            endpoint: args.endpoint_a.clone(),
            client_id: args.client_id_a.clone(),
            root: root_a,
            state: state_a,
        },
        endpoint_b: EndpointConfig {
            label: "agent_b",
            endpoint: args.endpoint_b.clone(),
            client_id: args.client_id_b.clone(),
            root: root_b,
            state: state_b,
        },
        timeouts,
        max_active_transfers: args.max_active_transfers,
    })
}

fn prepare_private_directory(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(HarnessError::InvalidConfiguration(
            "workspace and state paths must be absolute",
        ));
    }
    fs::create_dir_all(path).map_err(|error| io_error(path, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| io_error(path, error))?;
    }
    let canonical = path.canonicalize().map_err(|error| io_error(path, error))?;
    let mut entries = fs::read_dir(&canonical).map_err(|error| io_error(&canonical, error))?;
    if entries
        .next()
        .transpose()
        .map_err(|error| io_error(&canonical, error))?
        .is_some()
    {
        return Err(HarnessError::InvalidConfiguration(
            "workspace and state directories must start empty",
        ));
    }
    Ok(canonical)
}

fn canonical_executable(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(HarnessError::InvalidConfiguration(
            "agent, hook, and observer paths must be absolute",
        ));
    }
    let canonical = path.canonicalize().map_err(|error| io_error(path, error))?;
    let metadata = fs::metadata(&canonical).map_err(|error| io_error(&canonical, error))?;
    if !metadata.is_file() {
        return Err(HarnessError::InvalidConfiguration(
            "agent, hook, and observer paths must be files",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(HarnessError::InvalidConfiguration(
                "agent, hook, and observer files must be executable",
            ));
        }
    }
    Ok(canonical)
}

fn ensure_independent_observer(observer: &PinnedExecutable, executables: &[&Path]) -> Result<()> {
    #[cfg(unix)]
    {
        for executable in executables {
            if observer.source_is_same_file(executable)? {
                return Err(HarnessError::InvalidConfiguration(
                    "effect observer must be independent from agents and action hooks",
                ));
            }
        }
    }
    Ok(())
}

fn ensure_disjoint(paths: &[&Path]) -> Result<()> {
    for (index, left) in paths.iter().enumerate() {
        for right in &paths[index + 1..] {
            if left.starts_with(right) || right.starts_with(left) {
                return Err(HarnessError::InvalidConfiguration(
                    "workspace and state paths must not overlap",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    fn hook_config(temporary: &Path, timeouts: Timeouts) -> HarnessConfig {
        let observer = temporary.join("observer.sh");
        if !observer.exists() {
            write_executable(&observer, "#!/bin/sh\nexit 1\n");
        }
        HarnessConfig {
            workspace_id: "10000000-0000-4000-8000-000000000002".to_owned(),
            agent_binary: "/usr/bin/false".into(),
            reconnect_hook: temporary.join("hook.sh"),
            app_restart_hook: temporary.join("hook.sh"),
            effect_observer: PinnedExecutable::pin(&observer).expect("pin effect observer"),
            endpoint_a: EndpointConfig {
                label: "agent_a",
                endpoint: "ws://127.0.0.1:1/api/user/workspace-sync/v2".to_owned(),
                client_id: "10000000-0000-4000-8000-000000000003".to_owned(),
                root: temporary.join("root-a"),
                state: temporary.join("state-a"),
            },
            endpoint_b: EndpointConfig {
                label: "agent_b",
                endpoint: "ws://127.0.0.1:2/api/user/workspace-sync/v2".to_owned(),
                client_id: "10000000-0000-4000-8000-000000000004".to_owned(),
                root: temporary.join("root-b"),
                state: temporary.join("state-b"),
            },
            timeouts,
            max_active_transfers: 1,
        }
    }

    fn hook_timeouts(hook: Duration) -> Timeouts {
        Timeouts {
            startup: Duration::from_secs(1),
            checkpoint: Duration::from_secs(1),
            sample_interval: Duration::from_millis(10),
            hook,
            term_grace: Duration::from_millis(20),
            kill: Duration::from_secs(1),
        }
    }

    fn prepare_hook(temporary: &Path) -> PathBuf {
        let hook = temporary.join("hook.sh");
        fs::write(
            &hook,
            "#!/bin/sh\ntrap '' TERM\nwhile :; do /bin/sleep 1; done\n",
        )
        .expect("write hook");
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).expect("chmod hook");
        hook
    }

    fn write_executable(path: &Path, body: &str) {
        fs::write(path, body).expect("write executable fixture");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("chmod fixture");
    }

    fn evidence_run_id(case: &str) -> String {
        format!(
            "{case}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        )
    }

    #[tokio::test]
    async fn cancelled_hook_records_exact_failure_and_bounded_cleanup() {
        let temporary = tempfile::tempdir().expect("temporary hook");
        let hook = prepare_hook(temporary.path());
        let timeouts = hook_timeouts(Duration::from_secs(1));
        let config = hook_config(temporary.path(), timeouts);
        let context = effect_context(&config, (101, 202)).expect("effect context");
        let run_id = evidence_run_id("hook-cancel");
        let evidence = EvidenceWriter::create(&run_id, b"eyJ9.e30.c2ln").expect("evidence");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let mut sequence = 0;
        let error = run_hook(
            "reconnect_hook",
            EffectAction::Reconnect,
            &hook,
            &config,
            (101, 202),
            &context,
            timeouts,
            &evidence,
            &cancellation,
            &mut sequence,
        )
        .await
        .expect_err("cancelled hook");
        assert!(error.to_string().contains("cancelled"));
        let process =
            fs::read_to_string(evidence.root().join("process.jsonl")).expect("process evidence");
        assert!(process.contains("effect hook cancelled"));
        assert!(process.contains("reaped_after_error"));
        fs::remove_dir_all(evidence.root()).expect("remove evidence");
    }

    #[tokio::test]
    async fn timed_out_hook_records_exact_failure_and_bounded_cleanup() {
        let temporary = tempfile::tempdir().expect("temporary hook");
        let hook = prepare_hook(temporary.path());
        let timeouts = hook_timeouts(Duration::from_millis(30));
        let config = hook_config(temporary.path(), timeouts);
        let context = effect_context(&config, (101, 202)).expect("effect context");
        let run_id = evidence_run_id("hook-timeout");
        let evidence = EvidenceWriter::create(&run_id, b"eyJ9.e30.c2ln").expect("evidence");
        let mut sequence = 0;
        let error = run_hook(
            "app_restart_hook",
            EffectAction::AppRestart,
            &hook,
            &config,
            (101, 202),
            &context,
            timeouts,
            &evidence,
            &CancellationToken::new(),
            &mut sequence,
        )
        .await
        .expect_err("timed out hook");
        assert!(error.to_string().contains("timed out"));
        let process =
            fs::read_to_string(evidence.root().join("process.jsonl")).expect("process evidence");
        assert!(process.contains("operation timed out: effect hook"));
        assert!(process.contains("reaped_after_timeout"));
        fs::remove_dir_all(evidence.root()).expect("remove evidence");
    }

    #[tokio::test]
    async fn cleanup_timeout_is_persisted_with_exact_group_evidence_before_return() {
        let mut child = OwnedChild::spawn(ProcessSpec::control(
            "cleanup-failure",
            "/bin/sh",
            [
                "-c",
                "/bin/sh -c 'trap \"\" TERM; while :; do /bin/sleep 1; done' >/dev/null 2>&1 & printf 'ready\\n'; exit 0",
            ],
        ))
        .expect("spawn leader with resistant descendant");
        let pid = child.pid().as_raw_pid();
        let pgid = child.pgid().as_raw_pid();
        let stdout = child.take_stdout().expect("leader stdout");
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut ready = String::new();
        tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut ready)
            .await
            .expect("leader readiness");
        assert_eq!(ready, "ready\n");
        child.wait().await.expect("reap leader");

        let error = child
            .ensure_group_empty(Duration::ZERO, Duration::ZERO)
            .await
            .expect_err("zero cleanup bound must expose resistant group timeout");
        let run_id = evidence_run_id("cleanup-failure");
        let evidence = EvidenceWriter::create(&run_id, b"eyJ9.e30.c2ln").expect("evidence");
        let mut sequence = 0;
        record_cleanup_error(
            &evidence,
            "cleanup-failure",
            "hook_cleanup_failed",
            "forced cleanup regression",
            &error,
            pid,
            pgid,
            &mut sequence,
        )
        .expect("persist cleanup failure before returning it");

        let process =
            fs::read_to_string(evidence.root().join("process.jsonl")).expect("process evidence");
        assert!(process.contains(&format!("\"pid\":{pid}")));
        assert!(process.contains(&format!("\"pgid\":{pgid}")));
        assert!(process.contains("\"reason\":\"forced cleanup regression\""));
        assert!(process.contains("\"term_attempted\":true"));
        assert!(process.contains("\"kill_attempted\":true"));
        assert!(process.contains("\"descendants_present\":true"));
        assert!(process.contains("\"leader_reaped\":true"));
        assert!(process.contains("\"group_empty\":false"));
        assert!(process.contains("\"cleanup_timed_out\":true"));

        for _ in 0..100 {
            if rustix::process::test_kill_process_group(child.pgid())
                == Err(rustix::io::Errno::SRCH)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            rustix::process::test_kill_process_group(child.pgid()),
            Err(rustix::io::Errno::SRCH),
            "the failed bounded proof still sent KILL and left no owned process"
        );
        fs::remove_dir_all(evidence.root()).expect("remove evidence");
    }

    #[tokio::test]
    async fn successful_hook_records_resistant_descendant_term_kill_and_empty_group() {
        let temporary = tempfile::tempdir().expect("temporary hook");
        let timeouts = hook_timeouts(Duration::from_secs(10));
        let config = hook_config(temporary.path(), timeouts);
        let context = effect_context(&config, (101, 202)).expect("effect context");
        let receipt = EffectReceipt::observed_transition(
            EffectAction::Reconnect,
            context.clone(),
            EffectIdentity {
                pid: Some(301),
                generation: Some(7),
            },
            EffectIdentity {
                pid: Some(301),
                generation: Some(8),
            },
        );
        let hook = temporary.path().join("resistant-hook.sh");
        let ready = temporary.path().join("descendant-ready");
        write_executable(
            &hook,
            &format!(
                "#!/bin/sh\n/bin/sh -c 'trap \"\" TERM; : > \"$1\"; while :; do /bin/sleep 1; done' sh '{}' >/dev/null 2>&1 &\nwhile [ ! -f '{}' ]; do /bin/sleep 0.01; done\nprintf '%s\\n' '{}'\n",
                ready.display(),
                ready.display(),
                serde_json::to_string(&receipt).expect("receipt JSON")
            ),
        );
        let run_id = evidence_run_id("hook-resistant-descendant");
        let evidence = EvidenceWriter::create(&run_id, b"eyJ9.e30.c2ln").expect("evidence");
        let mut sequence = 0;
        run_hook(
            "reconnect_hook",
            EffectAction::Reconnect,
            &hook,
            &config,
            (101, 202),
            &context,
            timeouts,
            &evidence,
            &CancellationToken::new(),
            &mut sequence,
        )
        .await
        .expect("receipt-producing hook is reaped with its descendants");

        let process =
            fs::read_to_string(evidence.root().join("process.jsonl")).expect("process evidence");
        assert!(process.contains("\"termination\":\"exit\""));
        assert!(process.contains("\"group_termination\":\"kill\""));
        assert!(process.contains("\"term_attempted\":true"));
        assert!(process.contains("\"kill_attempted\":true"));
        assert!(process.contains("\"descendants_present\":true"));
        assert!(process.contains("\"leader_reaped\":true"));
        assert!(process.contains("\"group_empty\":true"));
        fs::remove_dir_all(evidence.root()).expect("remove evidence");
    }

    #[tokio::test]
    async fn fabricated_receipt_fails_when_independent_observation_is_unchanged() {
        let temporary = tempfile::tempdir().expect("temporary hook");
        let timeouts = hook_timeouts(Duration::from_secs(10));
        let mut config = hook_config(temporary.path(), timeouts);
        let context = effect_context(&config, (101, 202)).expect("effect context");
        let unchanged = EffectObservation::new(
            EffectAction::Reconnect,
            context.clone(),
            EffectIdentity {
                pid: Some(301),
                generation: Some(7),
            },
        );
        let fabricated = EffectReceipt::observed_transition(
            EffectAction::Reconnect,
            context,
            unchanged.identity,
            EffectIdentity {
                pid: Some(301),
                generation: Some(8),
            },
        );
        let observer = temporary.path().join("observer.sh");
        write_executable(
            &observer,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\n",
                serde_json::to_string(&unchanged).expect("observation JSON")
            ),
        );
        config.effect_observer =
            PinnedExecutable::pin(&observer).expect("pin unchanged effect observer");
        let hook = temporary.path().join("fabricated-hook.sh");
        write_executable(
            &hook,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\n",
                serde_json::to_string(&fabricated).expect("receipt JSON")
            ),
        );
        let run_id = evidence_run_id("fabricated-effect");
        let evidence = EvidenceWriter::create(&run_id, b"eyJ9.e30.c2ln").expect("evidence");
        let mut sequence = 0;
        let error = run_observed_hook(
            "reconnect_hook",
            EffectAction::Reconnect,
            &hook,
            &config,
            (101, 202),
            timeouts,
            &evidence,
            &CancellationToken::new(),
            &mut sequence,
        )
        .await
        .expect_err("self-attested change cannot override unchanged observation");
        assert!(error.to_string().contains("independent observations"));
        let process =
            fs::read_to_string(evidence.root().join("process.jsonl")).expect("process evidence");
        assert!(process.contains("effect_validation_failed"));
        assert!(process.contains("does not match independent observations"));
        fs::remove_dir_all(evidence.root()).expect("remove evidence");
    }

    #[tokio::test]
    async fn replacing_observer_path_cannot_forge_independent_convergence() {
        let temporary = tempfile::tempdir().expect("temporary hook");
        let timeouts = hook_timeouts(Duration::from_secs(10));
        let mut config = hook_config(temporary.path(), timeouts);
        let context = effect_context(&config, (101, 202)).expect("effect context");
        let before = EffectObservation::new(
            EffectAction::Reconnect,
            context.clone(),
            EffectIdentity {
                pid: Some(301),
                generation: Some(7),
            },
        );
        let forged_after = EffectObservation::new(
            EffectAction::Reconnect,
            context.clone(),
            EffectIdentity {
                pid: Some(301),
                generation: Some(8),
            },
        );
        let fabricated = EffectReceipt::observed_transition(
            EffectAction::Reconnect,
            context,
            before.identity,
            forged_after.identity,
        );
        let observer_marker = format!("pinned-observer-marker-{}", std::process::id());
        let observer = temporary.path().join("observer.sh");
        write_executable(
            &observer,
            &format!(
                "#!/bin/sh\n# {observer_marker}\nprintf '%s\\n' '{}'\n",
                serde_json::to_string(&before).expect("before observation JSON")
            ),
        );
        config.effect_observer =
            PinnedExecutable::pin(&observer).expect("pin original effect observer");
        let pinned_descriptor = config.effect_observer.raw_fd();
        let replacement = temporary.path().join("replacement-observer.sh");
        write_executable(
            &replacement,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\n",
                serde_json::to_string(&forged_after).expect("forged observation JSON")
            ),
        );
        let hook = temporary.path().join("replacement-hook.sh");
        write_executable(
            &hook,
            &format!(
                "#!/bin/sh\nfor descriptor in /dev/fd/*; do\n  if [ -f \"$descriptor\" ] && /usr/bin/grep -q '{observer_marker}' \"$descriptor\" 2>/dev/null; then exit 93; fi\ndone\n/bin/rm -f '{}'\n/bin/mv '{}' '{}'\nprintf '%s\\n' '{}'\n",
                observer.display(),
                replacement.display(),
                observer.display(),
                serde_json::to_string(&fabricated).expect("fabricated receipt JSON")
            ),
        );
        let run_id = evidence_run_id("replaced-observer-effect");
        let evidence = EvidenceWriter::create(&run_id, b"eyJ9.e30.c2ln").expect("evidence");
        let mut sequence = 0;
        let error = run_observed_hook(
            "reconnect_hook",
            EffectAction::Reconnect,
            &hook,
            &config,
            (101, 202),
            timeouts,
            &evidence,
            &CancellationToken::new(),
            &mut sequence,
        )
        .await
        .expect_err("replacement observer cannot forge independent convergence");
        assert!(error.to_string().contains("independent observations"));
        let process =
            fs::read_to_string(evidence.root().join("process.jsonl")).expect("process evidence");
        assert!(process.contains("effect_validation_failed"));
        assert!(process.contains("does not match independent observations"));
        fs::remove_dir_all(evidence.root()).expect("remove evidence");
        drop(config);
        assert_eq!(
            unsafe { libc::fcntl(pinned_descriptor, libc::F_GETFD) },
            -1,
            "dropping the harness config must close its observer pin"
        );
    }
}
