//! Read-only native harness adapters used by `ai-memory run`.

mod harness;
mod repository;
mod transcript;

pub use harness::{LaunchMode, LaunchPlan, ManagedHarness, build_launch_plan};
pub use repository::{RepositoryIdentity, inspect_repository};
pub use transcript::{
    ExportedTranscript, discover_native_session, export_transcript, wait_for_transcript_flush,
};
