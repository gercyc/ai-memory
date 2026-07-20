//! Opt-in managed cross-harness launcher.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use ai_memory_core::{
    FinishManagedRunRequest, FinishManagedRunResponse, LinkManagedRunRequest, ManagedRunStatus,
    PrepareManagedRunRequest, PrepareManagedRunResponse,
};
use ai_memory_workstream::{
    ExportedTranscript, ManagedHarness, build_launch_plan, discover_native_session,
    export_transcript, inspect_repository, wait_for_transcript_flush,
};
use anyhow::{Context as _, Result, anyhow};
use tokio::process::Command;

use crate::cli::{RunArgs, RunHarnessChoice};
use crate::commands::{path_util, resolve_project_name};
use crate::config::Config;
use crate::http_client::{ServerEndpoint, get_json, post_empty, post_json, post_json_no_content};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const IMPORT_BATCH_EVENTS: usize = 400;
const IMPORT_BATCH_BYTES: usize = 1024 * 1024;

/// Run one native harness and return its exact process exit code.
pub async fn run(config: &Config, args: RunArgs) -> Result<i32> {
    let harness = managed_harness(args.harness);
    let cwd = std::env::current_dir().context("getting managed run working directory")?;
    let repository = inspect_repository(&cwd)?;
    let project = resolve_project_name(config, args.project.as_deref())?;
    let endpoint = ServerEndpoint::from_config_resolving_auth(config).await;
    let prepare = PrepareManagedRunRequest {
        workspace: args.workspace,
        project,
        cwd: repository.cwd.to_string_lossy().into_owned(),
        repo_fingerprint: repository.repo_fingerprint,
        worktree_fingerprint: repository.worktree_fingerprint,
        agent: harness.agent_kind(),
        workstream: args.workstream,
        new_workstream: args.new_workstream,
        lease_owner: lease_owner(),
    };
    let prepared: PrepareManagedRunResponse = post_json(&endpoint, "/workstream/runs", &prepare)
        .await
        .context("opening managed workstream; the agent was not started")?;
    let plan = build_launch_plan(
        harness,
        args.executable.map(PathBuf::into_os_string),
        args.native_args,
        prepared.native_session_id.as_deref(),
    )?;
    let run_path = format!("/workstream/runs/{}", prepared.run_id);
    if let Some(native_session_id) = &plan.expected_session_id {
        post_json_no_content(
            &endpoint,
            &format!("{run_path}/link"),
            &LinkManagedRunRequest {
                native_session_id: native_session_id.clone(),
            },
        )
        .await
        .context("linking the managed native session; the agent was not started")?;
    }

    let started_at = SystemTime::now();
    let child = Command::new(&plan.program)
        .args(&plan.args)
        .current_dir(&repository.cwd)
        .env("AI_MEMORY_RUN_ID", prepared.run_id.to_string())
        .env(
            "AI_MEMORY_WORKSTREAM_ID",
            prepared.workstream_id.to_string(),
        )
        .env("AI_MEMORY_HOOK_URL", endpoint.build_url(""))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();

    let mut child = match child {
        Ok(child) => child,
        Err(spawn_error) => {
            let spawn_message = spawn_error.to_string();
            let request = FinishManagedRunRequest {
                native_session_id: plan.expected_session_id,
                source_cursor: prepared.source_cursor,
                events: Vec::new(),
                complete: true,
                checkpoint: repository.checkpoint,
                losses: vec![format!(
                    "native process could not be started: {spawn_message}"
                )],
                exit_code: None,
            };
            let _ = finish_with_retry(&endpoint, &run_path, &request).await;
            return Err(anyhow!(spawn_message)).context(format!(
                "starting managed {} executable {}",
                harness.as_str(),
                plan.program.to_string_lossy()
            ));
        }
    };

    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let status = loop {
        tokio::select! {
            result = child.wait() => break result.context("waiting for managed harness")?,
            _ = heartbeat.tick() => {
                if let Err(error) = post_empty(&endpoint, &format!("{run_path}/heartbeat")).await {
                    eprintln!("ai-memory: managed workstream heartbeat failed: {error}");
                }
            }
        }
    };
    let exit_code = status.code().unwrap_or(1);

    let server_status = get_json::<ManagedRunStatus>(&endpoint, &run_path, &[])
        .await
        .ok();
    let home = config
        .home_dir
        .as_deref()
        .map(PathBuf::from)
        .or_else(path_util::home_dir)
        .context("locating the home directory for native transcript import")?;
    let discovered_session = if plan.expected_session_id.is_none() {
        discover_native_session(
            harness,
            &home,
            &repository.cwd,
            plan.session_dir.as_deref(),
            started_at,
        )
        .await?
    } else {
        None
    };
    let native_session_id = plan
        .expected_session_id
        .clone()
        .or(discovered_session)
        .or_else(|| {
            server_status
                .as_ref()
                .and_then(|status| status.native_session_id.clone())
        });
    let source_cursor = if native_session_id.as_deref() == prepared.native_session_id.as_deref() {
        prepared.source_cursor.as_deref()
    } else {
        None
    };
    let transcript = export_after_flush(
        harness,
        &home,
        &repository.cwd,
        plan.session_dir.as_deref(),
        native_session_id.as_deref(),
        source_cursor,
    )
    .await;
    let checkpoint = inspect_repository(&repository.cwd)
        .map(|identity| identity.checkpoint)
        .unwrap_or(repository.checkpoint);
    let imported = import_batches(
        &endpoint,
        &run_path,
        transcript,
        checkpoint,
        Some(exit_code),
    )
    .await?;

    if prepared.sync_through > prepared.sync_after
        && !server_status.is_some_and(|status| status.context_delivered)
    {
        eprintln!(
            "ai-memory: this harness did not acknowledge its managed context packet; refresh its ai-memory hooks before the next run"
        );
    }
    eprintln!(
        "ai-memory: workstream '{}' saved {imported} new event(s)",
        prepared.workstream_name
    );
    Ok(exit_code)
}

async fn export_after_flush(
    harness: ManagedHarness,
    home: &std::path::Path,
    cwd: &std::path::Path,
    session_dir: Option<&std::path::Path>,
    native_session_id: Option<&str>,
    source_cursor: Option<&str>,
) -> ExportedTranscript {
    let Some(native_session_id) = native_session_id else {
        return ExportedTranscript {
            losses: vec![
                "native session id could not be discovered; transcript was not imported".into(),
            ],
            ..ExportedTranscript::default()
        };
    };
    if let Err(error) =
        wait_for_transcript_flush(harness, home, cwd, session_dir, native_session_id).await
    {
        eprintln!("ai-memory: transcript flush check failed: {error}");
    }
    match export_transcript(
        harness,
        home,
        cwd,
        session_dir,
        native_session_id,
        source_cursor,
    )
    .await
    {
        Ok(export) => export,
        Err(error) => ExportedTranscript {
            native_session_id: native_session_id.to_string(),
            source_cursor: source_cursor.map(str::to_string),
            losses: vec![format!("native transcript import failed: {error}")],
            events: Vec::new(),
        },
    }
}

async fn import_batches(
    endpoint: &ServerEndpoint,
    run_path: &str,
    transcript: ExportedTranscript,
    checkpoint: ai_memory_core::WorkstreamCheckpoint,
    exit_code: Option<i32>,
) -> Result<usize> {
    let mut imported = 0;
    let mut batches = event_batches(transcript.events).into_iter().peekable();
    while let Some(batch) = batches.next() {
        let complete = batches.peek().is_none();
        let request = FinishManagedRunRequest {
            native_session_id: nonempty_session(&transcript.native_session_id),
            source_cursor: complete.then(|| transcript.source_cursor.clone()).flatten(),
            events: batch,
            complete,
            checkpoint: checkpoint.clone(),
            losses: if complete {
                transcript.losses.clone()
            } else {
                Vec::new()
            },
            exit_code: complete.then_some(exit_code).flatten(),
        };
        imported += finish_with_retry(endpoint, run_path, &request)
            .await?
            .imported_events;
    }
    Ok(imported)
}

fn event_batches(
    events: Vec<ai_memory_core::NewWorkstreamEvent>,
) -> Vec<Vec<ai_memory_core::NewWorkstreamEvent>> {
    let mut batches = Vec::new();
    let mut batch = Vec::new();
    let mut bytes = 0_usize;
    for event in events {
        let event_bytes = serde_json::to_vec(&event).map_or(IMPORT_BATCH_BYTES, |raw| raw.len());
        if !batch.is_empty()
            && (batch.len() >= IMPORT_BATCH_EVENTS
                || bytes.saturating_add(event_bytes) > IMPORT_BATCH_BYTES)
        {
            batches.push(std::mem::take(&mut batch));
            bytes = 0;
        }
        bytes = bytes.saturating_add(event_bytes);
        batch.push(event);
    }
    batches.push(batch);
    batches
}

async fn finish_with_retry(
    endpoint: &ServerEndpoint,
    run_path: &str,
    request: &FinishManagedRunRequest,
) -> Result<FinishManagedRunResponse> {
    let path = format!("{run_path}/finish");
    let mut last_error = None;
    for attempt in 0..3 {
        match post_json(endpoint, &path, request).await {
            Ok(response) => return Ok(response),
            Err(error) => last_error = Some(error),
        }
        tokio::time::sleep(Duration::from_millis(250 * (attempt + 1))).await;
    }
    Err(last_error.unwrap_or_else(|| anyhow!("managed finish failed")))
        .context("persisting the managed transcript; the native process has already exited")
}

fn nonempty_session(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn lease_owner() -> String {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_string());
    format!("{host}:{}", std::process::id())
}

const fn managed_harness(choice: RunHarnessChoice) -> ManagedHarness {
    match choice {
        RunHarnessChoice::Claude => ManagedHarness::Claude,
        RunHarnessChoice::Codex => ManagedHarness::Codex,
        RunHarnessChoice::OpenCode => ManagedHarness::OpenCode,
        RunHarnessChoice::Pi => ManagedHarness::Pi,
        RunHarnessChoice::Omp => ManagedHarness::Omp,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};

    use clap::Parser as _;

    use crate::cli::{Cli, Command as CliCommand};

    #[test]
    fn native_arguments_do_not_require_separator_and_remain_opaque() {
        let cli = Cli::try_parse_from([
            OsStr::new("ai-memory"),
            OsStr::new("run"),
            OsStr::new("--project"),
            OsStr::new("memory"),
            OsStr::new("codex"),
            OsStr::new("--yolo"),
            OsStr::new("-m"),
            OsStr::new("gpt-5"),
            OsStr::new("continue here"),
        ])
        .unwrap();
        let CliCommand::Run(args) = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(args.project.as_deref(), Some("memory"));
        assert_eq!(
            args.native_args,
            ["--yolo", "-m", "gpt-5", "continue here"]
                .map(OsString::from)
                .to_vec()
        );
    }

    #[test]
    fn opencode_name_and_native_flags_parse_without_separator() {
        let cli = Cli::try_parse_from([
            OsStr::new("ai-memory"),
            OsStr::new("run"),
            OsStr::new("opencode"),
            OsStr::new("run"),
            OsStr::new("--model"),
            OsStr::new("provider/model"),
            OsStr::new("continue here"),
        ])
        .unwrap();
        let CliCommand::Run(args) = cli.command else {
            panic!("expected run command");
        };
        assert!(matches!(
            args.harness,
            crate::cli::RunHarnessChoice::OpenCode
        ));
        assert_eq!(
            args.native_args,
            ["run", "--model", "provider/model", "continue here"]
                .map(OsString::from)
                .to_vec()
        );
    }
}
