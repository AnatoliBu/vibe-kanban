//! Custom CLI agent executor
//!
//! Allows users to configure arbitrary CLI tools as coding agents
//! through profiles.json without code changes.

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use command_group::AsyncCommandGroup;
use derivative::Derivative;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, process::Command};
use ts_rs::TS;
use workspace_utils::msg_store::MsgStore;

use crate::{
    approvals::ExecutorApprovalService,
    command::{CmdOverrides, CommandBuilder, apply_overrides},
    env::ExecutionEnv,
    executors::{
        AppendPrompt, AvailabilityInfo, ExecutorError, SpawnedChild, StandardCodingAgentExecutor,
        acp::AcpAgentHarness,
    },
};

/// How to pass the prompt to the CLI tool
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default, TS, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PromptMode {
    /// Write prompt to stdin (default)
    /// Example: echo "prompt" | tool
    #[default]
    Stdin,
    /// Pass prompt as a named argument
    /// Example: tool --prompt "prompt"
    /// Requires `prompt_arg` to be set
    Arg,
    /// Pass prompt as the last positional argument
    /// Example: tool "prompt"
    LastPositional,
}

/// Custom CLI agent configuration
///
/// Allows configuring arbitrary CLI tools as coding agents.
/// Supports both simple stdin-based tools and ACP-compatible agents.
#[derive(Derivative, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[derivative(Debug, PartialEq)]
pub struct Custom {
    /// CLI command to execute
    /// Example: "npx -y @cline/cli" or "/usr/local/bin/my-tool"
    #[schemars(description = "CLI command to run (e.g., 'npx -y @cline/cli' or '/path/to/tool')")]
    pub command: String,

    /// How to pass the prompt to the tool
    #[schemars(description = "How to pass prompt: STDIN (pipe), ARG (--arg), or LAST_POSITIONAL")]
    #[serde(default)]
    pub prompt_mode: PromptMode,

    /// Argument name for prompt when prompt_mode is ARG
    /// Example: "--message" or "-p"
    #[schemars(description = "Argument flag for prompt (required when prompt_mode is ARG)")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_arg: Option<String>,

    /// Enable ACP (Agent Client Protocol) mode
    /// Set to true for tools that implement the ACP protocol
    #[schemars(description = "Enable ACP protocol for compatible tools")]
    #[serde(default)]
    pub acp: bool,

    /// Session namespace for ACP mode
    /// Used to organize session files
    #[schemars(description = "Session namespace for ACP (default: 'custom_sessions')")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_namespace: Option<String>,

    /// Extra text appended to the prompt
    #[serde(default)]
    pub append_prompt: AppendPrompt,

    /// Command overrides (base_command_override, additional_params, env)
    #[serde(flatten)]
    pub cmd: CmdOverrides,

    #[serde(skip)]
    #[ts(skip)]
    #[derivative(Debug = "ignore", PartialEq = "ignore")]
    pub approvals: Option<Arc<dyn ExecutorApprovalService>>,
}

impl Custom {
    /// Validate configuration at load time
    /// Returns an error if the configuration is invalid
    pub fn validate(&self) -> Result<(), String> {
        // Validate command is not empty or whitespace-only
        if self.command.trim().is_empty() {
            return Err("command field cannot be empty or whitespace-only".to_string());
        }

        // Validate prompt_arg requirement for ARG mode
        if self.prompt_mode == PromptMode::Arg && self.prompt_arg.is_none() {
            return Err("prompt_arg is required when prompt_mode is ARG".to_string());
        }

        Ok(())
    }

    fn build_command_builder(&self) -> CommandBuilder {
        let builder = CommandBuilder::new(&self.command);
        apply_overrides(builder, &self.cmd)
    }

    fn harness(&self) -> AcpAgentHarness {
        let namespace = self
            .session_namespace
            .clone()
            .unwrap_or_else(|| "custom_sessions".to_string());
        AcpAgentHarness::with_session_namespace(namespace)
    }

    /// Spawn a simple (non-ACP) process
    async fn spawn_simple(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let mut command_builder = self.build_command_builder();

        // Add prompt based on mode
        match &self.prompt_mode {
            PromptMode::Stdin => {
                // Prompt will be written to stdin after spawn
            }
            PromptMode::Arg => {
                // SAFETY: prompt_arg is guaranteed to exist by validate() method called at spawn
                let arg = self.prompt_arg.as_ref().unwrap();
                command_builder = command_builder.extend_params([arg.clone(), combined_prompt.clone()]);
            }
            PromptMode::LastPositional => {
                command_builder = command_builder.extend_params([combined_prompt.clone()]);
            }
        }

        let command_parts = command_builder.build_initial()?;
        let (program_path, args) = command_parts.into_resolved().await?;

        let mut command = Command::new(program_path);
        command
            .kill_on_drop(true)
            .current_dir(current_dir)
            .args(&args);

        // Set up stdin pipe only for stdin mode
        if matches!(self.prompt_mode, PromptMode::Stdin) {
            command.stdin(std::process::Stdio::piped());
        }

        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        env.clone()
            .with_profile(&self.cmd)
            .apply_to_command(&mut command);

        let mut child = command.group_spawn()?;

        // Write prompt to stdin if using stdin mode
        if matches!(self.prompt_mode, PromptMode::Stdin)
            && let Some(mut stdin) = child.inner().stdin.take()
        {
            stdin.write_all(combined_prompt.as_bytes()).await?;
            stdin.flush().await?;
            drop(stdin);
        }

        Ok(SpawnedChild::from(child))
    }

    /// Spawn an ACP-compatible process
    async fn spawn_acp(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let harness = self.harness();
        let command_parts = self.build_command_builder().build_initial()?;

        harness
            .spawn_with_command(
                current_dir,
                combined_prompt,
                command_parts,
                env,
                &self.cmd,
                self.approvals.clone(),
            )
            .await
    }

    /// Spawn follow-up for ACP
    async fn spawn_follow_up_acp(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let harness = self.harness();
        let command_parts = self.build_command_builder().build_follow_up(&[])?;

        harness
            .spawn_follow_up_with_command(
                current_dir,
                combined_prompt,
                session_id,
                command_parts,
                env,
                &self.cmd,
                self.approvals.clone(),
            )
            .await
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for Custom {
    fn use_approvals(&mut self, approvals: Arc<dyn ExecutorApprovalService>) {
        self.approvals = Some(approvals);
    }

    async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        // Validate configuration before spawning
        self.validate().map_err(|e| ExecutorError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            e,
        )))?;

        if self.acp {
            self.spawn_acp(current_dir, prompt, env).await
        } else {
            self.spawn_simple(current_dir, prompt, env).await
        }
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        if self.acp {
            self.spawn_follow_up_acp(current_dir, prompt, session_id, env)
                .await
        } else {
            // For non-ACP tools, follow-up is just a new spawn
            // (no session continuity)
            self.spawn_simple(current_dir, prompt, env).await
        }
    }

    fn normalize_logs(&self, msg_store: Arc<MsgStore>, worktree_path: &Path) {
        if self.acp {
            crate::executors::acp::normalize_logs(msg_store, worktree_path);
        }
        // For non-ACP tools, logs are passed through as-is
    }

    fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
        // Custom agents don't have a default MCP config path
        // Users can configure MCP in their tool's native config
        None
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        // Custom agents are always considered "found" since the command
        // existence is checked at runtime
        AvailabilityInfo::InstallationFound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_empty_command_fails() {
        let custom = Custom {
            command: "".to_string(),
            prompt_mode: PromptMode::Stdin,
            prompt_arg: None,
            acp: false,
            session_namespace: None,
            append_prompt: AppendPrompt::default(),
            cmd: CmdOverrides::default(),
            approvals: None,
        };
        assert!(custom.validate().is_err(), "Empty command should fail validation");
    }

    #[test]
    fn test_validate_whitespace_command_fails() {
        let custom = Custom {
            command: "   ".to_string(),
            prompt_mode: PromptMode::Stdin,
            prompt_arg: None,
            acp: false,
            session_namespace: None,
            append_prompt: AppendPrompt::default(),
            cmd: CmdOverrides::default(),
            approvals: None,
        };
        assert!(custom.validate().is_err(), "Whitespace-only command should fail validation");
    }

    #[test]
    fn test_validate_valid_command() {
        let custom = Custom {
            command: "echo hello".to_string(),
            prompt_mode: PromptMode::Stdin,
            prompt_arg: None,
            acp: false,
            session_namespace: None,
            append_prompt: AppendPrompt::default(),
            cmd: CmdOverrides::default(),
            approvals: None,
        };
        assert!(custom.validate().is_ok(), "Valid command should pass validation");
    }

    #[test]
    fn test_validate_arg_mode_without_prompt_arg_fails() {
        let custom = Custom {
            command: "tool".to_string(),
            prompt_mode: PromptMode::Arg,
            prompt_arg: None,
            acp: false,
            session_namespace: None,
            append_prompt: AppendPrompt::default(),
            cmd: CmdOverrides::default(),
            approvals: None,
        };
        assert!(custom.validate().is_err(), "ARG mode without prompt_arg should fail");
    }

    #[test]
    fn test_validate_arg_mode_with_prompt_arg_succeeds() {
        let custom = Custom {
            command: "tool".to_string(),
            prompt_mode: PromptMode::Arg,
            prompt_arg: Some("--message".to_string()),
            acp: false,
            session_namespace: None,
            append_prompt: AppendPrompt::default(),
            cmd: CmdOverrides::default(),
            approvals: None,
        };
        assert!(custom.validate().is_ok(), "ARG mode with prompt_arg should succeed");
    }

    #[test]
    fn test_stdin_mode_default() {
        let custom = Custom {
            command: "tool".to_string(),
            prompt_mode: PromptMode::Stdin,
            prompt_arg: None,
            acp: false,
            session_namespace: None,
            append_prompt: AppendPrompt::default(),
            cmd: CmdOverrides::default(),
            approvals: None,
        };
        assert_eq!(custom.prompt_mode, PromptMode::Stdin);
        assert!(custom.validate().is_ok());
    }

    #[test]
    fn test_last_positional_mode() {
        let custom = Custom {
            command: "tool".to_string(),
            prompt_mode: PromptMode::LastPositional,
            prompt_arg: None,
            acp: false,
            session_namespace: None,
            append_prompt: AppendPrompt::default(),
            cmd: CmdOverrides::default(),
            approvals: None,
        };
        assert_eq!(custom.prompt_mode, PromptMode::LastPositional);
        assert!(custom.validate().is_ok());
    }

    #[test]
    fn test_acp_mode_enabled() {
        let custom = Custom {
            command: "tool".to_string(),
            prompt_mode: PromptMode::Stdin,
            prompt_arg: None,
            acp: true,
            session_namespace: None,
            append_prompt: AppendPrompt::default(),
            cmd: CmdOverrides::default(),
            approvals: None,
        };
        assert!(custom.acp);
    }

    #[test]
    fn test_acp_mode_disabled_default() {
        let custom = Custom {
            command: "tool".to_string(),
            prompt_mode: PromptMode::Stdin,
            prompt_arg: None,
            acp: false,
            session_namespace: None,
            append_prompt: AppendPrompt::default(),
            cmd: CmdOverrides::default(),
            approvals: None,
        };
        assert!(!custom.acp);
    }

    #[test]
    fn test_session_namespace_custom() {
        let custom = Custom {
            command: "tool".to_string(),
            prompt_mode: PromptMode::Stdin,
            prompt_arg: None,
            acp: true,
            session_namespace: Some("my_sessions".to_string()),
            append_prompt: AppendPrompt::default(),
            cmd: CmdOverrides::default(),
            approvals: None,
        };
        assert_eq!(custom.session_namespace, Some("my_sessions".to_string()));
    }

    #[test]
    fn test_harness_default_namespace() {
        let custom = Custom {
            command: "tool".to_string(),
            prompt_mode: PromptMode::Stdin,
            prompt_arg: None,
            acp: true,
            session_namespace: None,
            append_prompt: AppendPrompt::default(),
            cmd: CmdOverrides::default(),
            approvals: None,
        };
        let _harness = custom.harness();
        // Verify harness creation works
    }

    #[test]
    fn test_build_command_builder() {
        let custom = Custom {
            command: "npx -y @cline/cli".to_string(),
            prompt_mode: PromptMode::Stdin,
            prompt_arg: None,
            acp: false,
            session_namespace: None,
            append_prompt: AppendPrompt::default(),
            cmd: CmdOverrides::default(),
            approvals: None,
        };
        let _builder = custom.build_command_builder();
        // Verify it doesn't panic
    }

    #[test]
    fn test_deserialization_basic() {
        let json = r#"{"command": "echo hello"}"#;
        let result: Result<Custom, _> = serde_json::from_str(json);
        assert!(result.is_ok(), "Valid command should deserialize");
        let custom = result.unwrap();
        assert_eq!(custom.command, "echo hello");
        assert_eq!(custom.prompt_mode, PromptMode::Stdin);
    }
}
