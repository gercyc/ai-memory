//! `ai-memory auto-improve` — review one session and apply durable wiki edits through the auto-improvement approval path.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::cli::AutoImproveArgs;
use crate::config::Config;
use crate::http_client::{ServerEndpoint, post_json};

/// Request sent to `POST /admin/auto-improve`.
#[derive(Serialize)]
struct AutoImproveRequest {
    workspace: String,
    project: String,
    session_id: String,
    min_observations: usize,
    min_session_duration_secs: u64,
    min_confidence: f32,
    max_input_tokens: usize,
    max_proposals_per_run: usize,
    include_raw_fallback: bool,
    proposal_actor: String,
    pending_path: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct StageResponse {
    run_id: String,
    approval_required: bool,
    approval_policy: String,
    session_id: String,
    summary: String,
    warnings: Vec<String>,
    rejected_candidates_count: usize,
    proposals: Vec<ProposalOutcome>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProposalOutcome {
    id: String,
    sidecar_path: String,
    status: String,
    page_id: Option<String>,
}

/// Run the `auto-improve` subcommand.
///
/// # Errors
/// Returns an error if the server is unreachable or rejects the review request.
pub async fn run(config: &Config, args: AutoImproveArgs) -> Result<()> {
    let endpoint = ServerEndpoint::from_config(config);
    let project = super::resolve_project_name(config, args.project.as_deref())?;
    let settings = &config.auto_improve;
    let request = AutoImproveRequest {
        workspace: args.workspace,
        project: project.clone(),
        session_id: args.session_id,
        min_observations: args.min_observations.unwrap_or(settings.min_observations),
        min_session_duration_secs: args
            .min_session_duration_secs
            .unwrap_or(settings.min_session_duration_secs),
        min_confidence: args.min_confidence.unwrap_or(settings.min_confidence),
        max_input_tokens: args.max_input_tokens.unwrap_or(settings.max_input_tokens),
        max_proposals_per_run: args.max_proposals.unwrap_or(settings.max_proposals_per_run),
        include_raw_fallback: args.include_raw_fallback || settings.include_raw_fallback,
        proposal_actor: settings.proposal_actor.clone(),
        pending_path: settings.pending_path.clone(),
    };

    let response: StageResponse = post_json(&endpoint, "/admin/auto-improve", &request).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else {
        if response.approval_required {
            println!(
                "Staged auto-improve run {} for manual approval",
                response.run_id
            );
        } else {
            println!("Auto-approved auto-improve run {}", response.run_id);
        }
        println!("Approval policy: {}", response.approval_policy);
        for proposal in &response.proposals {
            if let Some(page_id) = &proposal.page_id {
                println!(
                    "  - {} [{}] page {} ({})",
                    proposal.id, proposal.status, page_id, proposal.sidecar_path
                );
            } else {
                println!(
                    "  - {} [{}] ({})",
                    proposal.id, proposal.status, proposal.sidecar_path
                );
            }
        }
    }
    Ok(())
}
