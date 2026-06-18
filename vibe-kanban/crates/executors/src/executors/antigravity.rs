use std::{
    collections::{HashSet, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use derivative::Derivative;
use futures::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Command;
use ts_rs::TS;
use uuid::Uuid;
use workspace_utils::{
    command_ext::GroupSpawnNoWindowExt, log_msg::LogMsg, msg_store::MsgStore,
    path::make_path_relative,
};

use crate::{
    command::{CmdOverrides, CommandBuildError, CommandBuilder, CommandParts, apply_overrides},
    env::ExecutionEnv,
    executor_discovery::ExecutorDiscoveredOptions,
    executors::{
        AppendPrompt, AvailabilityInfo, BaseCodingAgent, ExecutorError, SpawnedChild,
        StandardCodingAgentExecutor,
    },
    logs::{
        ActionType, CommandRunResult, FileChange, NormalizedEntry, NormalizedEntryError,
        NormalizedEntryType, TokenUsageInfo, ToolResult, ToolStatus,
        utils::{
            EntryIndexProvider,
            patch::{self, add_normalized_entry, replace_normalized_entry},
            shell_command_parsing::CommandCategory,
        },
    },
    model_selector::{ModelInfo, ModelSelectorConfig, PermissionPolicy, ReasoningOption},
    profile::ExecutorConfig,
};

const DEFAULT_ANTIGRAVITY_CONTEXT_WINDOW: u32 = 1_048_576;
const ANTIGRAVITY_MODEL_DISCOVERY_TIMEOUT_SECS: u64 = 10;
const PROMPT_CHARS_MARKER: &str = "VIBE_KANBAN_AGY_PROMPT_CHARS=";
const CONVERSATION_ID_MARKER: &str = "VIBE_KANBAN_AGY_CONVERSATION_ID=";
const TRANSCRIPT_POLL_INTERVAL_MS: u64 = 500;
const TRANSCRIPT_FINISH_GRACE_POLLS: usize = 4;
const FALLBACK_ANTIGRAVITY_MODELS: &[&str] = &[
    "Gemini 3.5 Flash (Medium)",
    "Gemini 3.5 Flash (High)",
    "Gemini 3.5 Flash (Low)",
    "Gemini 3.1 Pro (Low)",
    "Gemini 3.1 Pro (High)",
    "Claude Sonnet 4.6 (Thinking)",
    "Claude Opus 4.6 (Thinking)",
    "GPT-OSS 120B (Medium)",
];

const AGY_WRAPPER_SCRIPT: &str = r#"
agy_bin=$1
agy_log=$2
agy_prompt_chars=$3
agy_workspace=$4
shift 4

printf '\nVIBE_KANBAN_AGY_PROMPT_CHARS=%s\n' "$agy_prompt_chars" >&2

cd "$agy_workspace" || exit 1

"$agy_bin" "$@" --log-file "$agy_log" &
agy_pid=$!
agy_marker="${agy_log}.conversation"

(
  while kill -0 "$agy_pid" 2>/dev/null; do
    if [ -f "$agy_log" ]; then
      found_id=$(sed -nE 's/.*(Created conversation |Print mode: conversation=)([0-9a-fA-F-]{36}).*/\2/p' "$agy_log" | tail -n 1)
      if [ -n "$found_id" ]; then
        printf '\nVIBE_KANBAN_AGY_CONVERSATION_ID=%s\n' "$found_id" >&2
        printf '%s' "$found_id" > "$agy_marker"
        exit 0
      fi
    fi
    sleep 1
  done
) &
agy_watcher_pid=$!

wait "$agy_pid"
status=$?
kill "$agy_watcher_pid" 2>/dev/null
wait "$agy_watcher_pid" 2>/dev/null

if [ -f "$agy_log" ]; then
  agy_conversation_id=$(cat "$agy_marker" 2>/dev/null || true)
  final_conversation_id=$(sed -nE 's/.*(Created conversation |Print mode: conversation=)([0-9a-fA-F-]{36}).*/\2/p' "$agy_log" | tail -n 1)
  if [ -n "$final_conversation_id" ] && [ "$final_conversation_id" != "$agy_conversation_id" ]; then
    printf '\nVIBE_KANBAN_AGY_CONVERSATION_ID=%s\n' "$final_conversation_id" >&2
  fi
  rm -f "$agy_log" "$agy_marker"
fi
rm -f "$agy_marker"

exit "$status"
"#;

#[derive(Derivative, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[derivative(Debug, PartialEq)]
pub struct AntiGravity {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        title = "Model",
        description = "Model name or ID passed to `agy --model`."
    )]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        title = "Effort",
        description = "Model effort variant from `agy models`, combined with the base model name for `agy --model`."
    )]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        title = "Dangerously Skip Permissions",
        description = "Pass `--dangerously-skip-permissions` to Antigravity CLI."
    )]
    pub dangerously_skip_permissions: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        title = "Sandbox",
        description = "Pass `--sandbox` to Antigravity CLI."
    )]
    pub sandbox: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        title = "Print Timeout",
        description = "Timeout passed to `agy --print-timeout`, for example `5m0s` or `30s`."
    )]
    pub print_timeout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        title = "Additional Directories",
        description = "Extra directories passed as repeated `agy --add-dir` values."
    )]
    pub add_dir: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        title = "Context Window",
        description = "Context window used for Vibe Kanban's context usage gauge. Antigravity CLI does not currently expose exact token usage, so usage is estimated."
    )]
    pub context_window: Option<u32>,
    #[serde(flatten)]
    pub cmd: CmdOverrides,
}

impl AntiGravity {
    fn build_command_builder(&self) -> Result<CommandBuilder, CommandBuildError> {
        let mut builder = CommandBuilder::new("agy");

        if self.dangerously_skip_permissions.unwrap_or(true) {
            builder = builder.extend_params(["--dangerously-skip-permissions"]);
        }

        if self.sandbox.unwrap_or(false) {
            builder = builder.extend_params(["--sandbox"]);
        }

        if let Some(model) = self.effective_model_arg() {
            builder = builder.extend_params(["--model", model.as_str()]);
        }

        if let Some(timeout) = &self.print_timeout {
            builder = builder.extend_params(["--print-timeout", timeout]);
        }

        if let Some(dirs) = &self.add_dir {
            for dir in dirs {
                builder = builder.extend_params(["--add-dir", dir]);
            }
        }

        apply_overrides(builder, &self.cmd)
    }

    fn effective_model_arg(&self) -> Option<String> {
        compose_antigravity_model_arg(self.model.as_deref(), self.effort.as_deref())
    }

    async fn spawn_internal(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let workspace_dir = current_dir
            .canonicalize()
            .unwrap_or_else(|_| current_dir.to_path_buf());
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let mut additional_args = Vec::new();

        additional_args.push("--add-dir".to_string());
        additional_args.push(workspace_dir.to_string_lossy().to_string());

        if let Some(session_id) = session_id {
            additional_args.push("--conversation".to_string());
            additional_args.push(session_id.to_string());
        }

        additional_args.push("-p".to_string());
        additional_args.push(combined_prompt.clone());

        let command_parts = self
            .build_command_builder()?
            .build_follow_up(&additional_args)?;

        spawn_agy(
            command_parts,
            &workspace_dir,
            env,
            &self.cmd,
            combined_prompt.chars().count(),
        )
        .await
    }

    fn effective_context_window(&self) -> u32 {
        self.context_window
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_ANTIGRAVITY_CONTEXT_WINDOW)
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for AntiGravity {
    fn apply_overrides(&mut self, executor_config: &ExecutorConfig) {
        if let Some(model_id) = &executor_config.model_id {
            self.model = Some(model_id.clone());
        }
        if let Some(reasoning_id) = &executor_config.reasoning_id {
            self.effort = Some(reasoning_id.clone());
        }
        if let Some(permission_policy) = executor_config.permission_policy.clone() {
            self.dangerously_skip_permissions =
                Some(matches!(permission_policy, PermissionPolicy::Auto));
        }
    }

    async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_internal(current_dir, prompt, None, env).await
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        _reset_to_message_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_internal(current_dir, prompt, Some(session_id), env)
            .await
    }

    fn normalize_logs(
        &self,
        msg_store: Arc<MsgStore>,
        worktree_path: &Path,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        normalize_logs(
            msg_store,
            self.effective_context_window(),
            antigravity_cli_home(&self.cmd),
            worktree_path.to_path_buf(),
        )
    }

    fn default_mcp_config_path(&self) -> Option<PathBuf> {
        antigravity_config_dir(&self.cmd).map(|dir| dir.join("mcp_config.json"))
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        let binary_found =
            workspace_utils::shell::resolve_executable_path_blocking("agy").is_some();
        if !binary_found {
            return AvailabilityInfo::NotFound;
        }

        if let Some(timestamp) = antigravity_cli_home(&self.cmd)
            .and_then(|home| std::fs::metadata(home.join("installation_id")).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
        {
            return AvailabilityInfo::LoginDetected {
                last_auth_timestamp: timestamp,
            };
        }

        AvailabilityInfo::InstallationFound
    }

    fn get_preset_options(&self) -> ExecutorConfig {
        let (model_id, reasoning_id) =
            preset_model_and_reasoning(self.model.as_deref(), self.effort.as_deref());
        ExecutorConfig {
            executor: BaseCodingAgent::AntiGravity,
            variant: None,
            model_id,
            agent_id: None,
            reasoning_id,
            permission_policy: Some(PermissionPolicy::Auto),
        }
    }

    async fn discover_options(
        &self,
        workdir: Option<&Path>,
        repo_path: Option<&Path>,
    ) -> Result<futures::stream::BoxStream<'static, json_patch::Patch>, ExecutorError> {
        let discovery_dir = workdir
            .or(repo_path)
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let mut initial_options = ExecutorDiscoveredOptions {
            model_selector: antigravity_model_selector_config(&fallback_antigravity_model_lines()),
            loading_models: true,
            ..Default::default()
        };
        initial_options.loading_agents = false;
        initial_options.loading_slash_commands = false;

        let initial_patch = patch::executor_discovered_options(initial_options);
        let cmd = self.cmd.clone();

        let discovery_stream = async_stream::stream! {
            let model_lines = discover_antigravity_model_lines(&cmd, &discovery_dir).await;
            let model_selector = antigravity_model_selector_config(&model_lines);
            yield patch::update_models(model_selector.models);
            yield patch::update_default_model(model_selector.default_model);
            yield patch::models_loaded();
        };

        Ok(Box::pin(
            futures::stream::once(async move { initial_patch }).chain(discovery_stream),
        ))
    }
}

async fn spawn_agy(
    command_parts: CommandParts,
    current_dir: &Path,
    env: &ExecutionEnv,
    cmd_overrides: &CmdOverrides,
    prompt_chars: usize,
) -> Result<SpawnedChild, ExecutorError> {
    let (program_path, args) = command_parts.into_resolved().await?;
    let log_path = antigravity_temp_log_path()?;

    let mut command = Command::new("sh");
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(current_dir)
        .arg("-c")
        .arg(AGY_WRAPPER_SCRIPT)
        .arg("vibe-kanban-agy")
        .arg(program_path)
        .arg(log_path)
        .arg(prompt_chars.to_string())
        .arg(current_dir.to_string_lossy().to_string())
        .args(args);

    env.clone()
        .with_profile(cmd_overrides)
        .apply_to_command(&mut command);

    Ok(command.group_spawn_no_window()?.into())
}

fn normalize_logs(
    msg_store: Arc<MsgStore>,
    context_window: u32,
    antigravity_home: Option<PathBuf>,
    worktree_path: PathBuf,
) -> Vec<tokio::task::JoinHandle<()>> {
    let entry_index = EntryIndexProvider::start_from(&msg_store);
    let prompt_tokens = Arc::new(AtomicU32::new(0));
    let transcript_assistant_message_seen = Arc::new(AtomicBool::new(false));
    let worktree_path = worktree_path.canonicalize().unwrap_or(worktree_path);
    let current_turn_started_at = Utc::now();

    let stdout_handle = normalize_stdout_logs(
        msg_store.clone(),
        entry_index.clone(),
        prompt_tokens.clone(),
        context_window,
        transcript_assistant_message_seen.clone(),
    );
    let stderr_handle = normalize_stderr_logs(
        msg_store,
        entry_index,
        prompt_tokens,
        context_window,
        antigravity_home,
        worktree_path,
        transcript_assistant_message_seen,
        current_turn_started_at,
    );

    vec![stdout_handle, stderr_handle]
}

fn normalize_stdout_logs(
    msg_store: Arc<MsgStore>,
    entry_index: EntryIndexProvider,
    prompt_tokens: Arc<AtomicU32>,
    context_window: u32,
    transcript_assistant_message_seen: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut stdout = msg_store.stdout_chunked_stream();
        let mut output = String::new();

        while let Some(Ok(chunk)) = stdout.next().await {
            output.push_str(&chunk);
        }

        let fallback_delay =
            TRANSCRIPT_POLL_INTERVAL_MS * (TRANSCRIPT_FINISH_GRACE_POLLS as u64 + 1);
        tokio::time::sleep(Duration::from_millis(fallback_delay)).await;
        if transcript_assistant_message_seen.load(Ordering::Relaxed) {
            return;
        }

        let content = strip_ansi_escapes::strip_str(output.trim_end()).to_string();
        if content.trim().is_empty() {
            return;
        }

        add_normalized_entry(
            &msg_store,
            &entry_index,
            NormalizedEntry {
                timestamp: None,
                entry_type: NormalizedEntryType::AssistantMessage,
                content,
                metadata: None,
            },
        );
        transcript_assistant_message_seen.store(true, Ordering::Relaxed);

        let total_tokens = prompt_tokens
            .load(Ordering::Relaxed)
            .saturating_add(estimate_tokens(&output));
        if total_tokens > 0 {
            add_token_usage_entry(&msg_store, &entry_index, total_tokens, context_window);
        }
    })
}

fn normalize_stderr_logs(
    msg_store: Arc<MsgStore>,
    entry_index: EntryIndexProvider,
    prompt_tokens: Arc<AtomicU32>,
    context_window: u32,
    antigravity_home: Option<PathBuf>,
    worktree_path: PathBuf,
    transcript_assistant_message_seen: Arc<AtomicBool>,
    current_turn_started_at: DateTime<Utc>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut stderr = msg_store.stderr_chunked_stream();
        let mut buffer = String::new();
        let mut stored_session_id = false;

        while let Some(Ok(chunk)) = stderr.next().await {
            buffer.push_str(&chunk);
            let complete_lines = buffer
                .split_inclusive('\n')
                .filter(|line| line.ends_with('\n'))
                .map(str::to_string)
                .collect::<Vec<_>>();

            for line in complete_lines {
                handle_stderr_line(
                    &line,
                    &msg_store,
                    &entry_index,
                    &prompt_tokens,
                    context_window,
                    antigravity_home.as_ref(),
                    &worktree_path,
                    &transcript_assistant_message_seen,
                    current_turn_started_at,
                    &mut stored_session_id,
                );
            }

            buffer = buffer.rsplit('\n').next().unwrap_or_default().to_string();
        }

        if !buffer.trim().is_empty() {
            handle_stderr_line(
                &buffer,
                &msg_store,
                &entry_index,
                &prompt_tokens,
                context_window,
                antigravity_home.as_ref(),
                &worktree_path,
                &transcript_assistant_message_seen,
                current_turn_started_at,
                &mut stored_session_id,
            );
        }
    })
}

fn handle_stderr_line(
    line: &str,
    msg_store: &Arc<MsgStore>,
    entry_index: &EntryIndexProvider,
    prompt_tokens: &Arc<AtomicU32>,
    context_window: u32,
    antigravity_home: Option<&PathBuf>,
    worktree_path: &Path,
    transcript_assistant_message_seen: &Arc<AtomicBool>,
    current_turn_started_at: DateTime<Utc>,
    stored_session_id: &mut bool,
) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }

    if let Some(raw) = trimmed.strip_prefix(PROMPT_CHARS_MARKER) {
        if let Ok(chars) = raw.parse::<u32>() {
            prompt_tokens.store(estimate_tokens_from_chars(chars), Ordering::Relaxed);
        }
        return;
    }

    if let Some(session_id) = trimmed.strip_prefix(CONVERSATION_ID_MARKER) {
        if !*stored_session_id && is_uuid_like(session_id) {
            msg_store.push_session_id(session_id.to_string());
            *stored_session_id = true;
            if let Some(antigravity_home) = antigravity_home {
                let transcript_path = antigravity_transcript_path(antigravity_home, session_id);
                tokio::spawn(tail_antigravity_transcript(
                    msg_store.clone(),
                    entry_index.clone(),
                    prompt_tokens.clone(),
                    context_window,
                    transcript_path,
                    worktree_path.to_path_buf(),
                    transcript_assistant_message_seen.clone(),
                    current_turn_started_at,
                ));
            }
        }
        return;
    }

    add_normalized_entry(
        msg_store,
        entry_index,
        NormalizedEntry {
            timestamp: None,
            entry_type: NormalizedEntryType::ErrorMessage {
                error_type: NormalizedEntryError::Other,
            },
            content: strip_ansi_escapes::strip_str(trimmed),
            metadata: None,
        },
    );
}

#[derive(Debug, Deserialize, Serialize)]
struct AntigravityTranscriptStep {
    step_index: Option<u64>,
    source: Option<String>,
    #[serde(rename = "type")]
    step_type: String,
    status: Option<String>,
    created_at: Option<String>,
    content: Option<String>,
    thinking: Option<String>,
    tool_calls: Option<Vec<AntigravityToolCall>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AntigravityToolCall {
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Clone)]
struct AntigravityToolInfo {
    entry_index: usize,
    tool_name: String,
    action_type: ActionType,
    content: String,
    expected_step_type: String,
}

struct AntigravityTranscriptState {
    entry_index: EntryIndexProvider,
    prompt_tokens: Arc<AtomicU32>,
    context_window: u32,
    worktree_path: String,
    transcript_assistant_message_seen: Arc<AtomicBool>,
    pending_tools: VecDeque<AntigravityToolInfo>,
    output_chars: usize,
    assistant_message_added: bool,
    token_usage_added: bool,
    current_turn_started_at: DateTime<Utc>,
}

impl AntigravityTranscriptState {
    fn new(
        entry_index: EntryIndexProvider,
        prompt_tokens: Arc<AtomicU32>,
        context_window: u32,
        worktree_path: PathBuf,
        transcript_assistant_message_seen: Arc<AtomicBool>,
        current_turn_started_at: DateTime<Utc>,
    ) -> Self {
        Self {
            entry_index,
            prompt_tokens,
            context_window,
            worktree_path: worktree_path.to_string_lossy().to_string(),
            transcript_assistant_message_seen,
            pending_tools: VecDeque::new(),
            output_chars: 0,
            assistant_message_added: false,
            token_usage_added: false,
            current_turn_started_at,
        }
    }

    fn handle_step(&mut self, step: AntigravityTranscriptStep, msg_store: &Arc<MsgStore>) {
        if self.should_skip_step(&step) {
            return;
        }

        if let Some(thinking) = step.thinking.as_deref().map(str::trim)
            && !thinking.is_empty()
        {
            self.output_chars = self.output_chars.saturating_add(thinking.chars().count());
            add_normalized_entry(
                msg_store,
                &self.entry_index,
                NormalizedEntry {
                    timestamp: step.created_at.clone(),
                    entry_type: NormalizedEntryType::Thinking,
                    content: thinking.to_string(),
                    metadata: serde_json::to_value(&step).ok(),
                },
            );
        }

        if let Some(tool_calls) = &step.tool_calls {
            for tool_call in tool_calls {
                self.handle_tool_call(tool_call, step.created_at.clone(), msg_store);
            }
        }

        if let Some(content) = step.content.as_deref().map(str::trim)
            && !content.is_empty()
        {
            self.output_chars = self.output_chars.saturating_add(content.chars().count());
            self.handle_content_step(&step, content, msg_store);
        }
    }

    fn should_skip_step(&self, step: &AntigravityTranscriptStep) -> bool {
        matches!(
            step.step_type.as_str(),
            "USER_INPUT" | "CONVERSATION_HISTORY" | "EPHEMERAL_MESSAGE" | "SYSTEM_MESSAGE"
        ) || matches!(step.source.as_deref(), Some("USER_EXPLICIT"))
            || self.is_previous_turn_step(step)
    }

    fn is_previous_turn_step(&self, step: &AntigravityTranscriptStep) -> bool {
        let Some(created_at) = step.created_at.as_deref() else {
            return false;
        };
        DateTime::parse_from_rfc3339(created_at)
            .map(|timestamp| timestamp.with_timezone(&Utc) < self.current_turn_started_at)
            .unwrap_or(false)
    }

    fn handle_tool_call(
        &mut self,
        tool_call: &AntigravityToolCall,
        timestamp: Option<String>,
        msg_store: &Arc<MsgStore>,
    ) {
        let action_type =
            action_type_for_antigravity_tool(&tool_call.name, &tool_call.args, &self.worktree_path);
        let content = antigravity_tool_content(&tool_call.name, &tool_call.args, &action_type);
        let expected_step_type = expected_step_type_for_tool(&tool_call.name);
        let entry = NormalizedEntry {
            timestamp,
            entry_type: NormalizedEntryType::ToolUse {
                tool_name: tool_call.name.clone(),
                action_type: action_type.clone(),
                status: ToolStatus::Created,
            },
            content: content.clone(),
            metadata: serde_json::to_value(tool_call).ok(),
        };
        let index = add_normalized_entry(msg_store, &self.entry_index, entry);
        self.pending_tools.push_back(AntigravityToolInfo {
            entry_index: index,
            tool_name: tool_call.name.clone(),
            action_type,
            content,
            expected_step_type,
        });
    }

    fn handle_content_step(
        &mut self,
        step: &AntigravityTranscriptStep,
        content: &str,
        msg_store: &Arc<MsgStore>,
    ) {
        if step.step_type == "PLANNER_RESPONSE" && matches!(step.source.as_deref(), Some("MODEL")) {
            if !self.assistant_message_added
                && self
                    .transcript_assistant_message_seen
                    .load(Ordering::Relaxed)
            {
                return;
            }
            self.assistant_message_added = true;
            self.transcript_assistant_message_seen
                .store(true, Ordering::Relaxed);
            add_normalized_entry(
                msg_store,
                &self.entry_index,
                NormalizedEntry {
                    timestamp: step.created_at.clone(),
                    entry_type: NormalizedEntryType::AssistantMessage,
                    content: content.to_string(),
                    metadata: serde_json::to_value(step).ok(),
                },
            );
            return;
        }

        if step.step_type == "ERROR_MESSAGE" {
            if self.replace_pending_tool(step, content, ToolStatus::Failed, msg_store) {
                return;
            }

            add_normalized_entry(
                msg_store,
                &self.entry_index,
                NormalizedEntry {
                    timestamp: step.created_at.clone(),
                    entry_type: NormalizedEntryType::ErrorMessage {
                        error_type: NormalizedEntryError::Other,
                    },
                    content: content.to_string(),
                    metadata: serde_json::to_value(step).ok(),
                },
            );
            return;
        }

        if is_antigravity_tool_result_step(&step.step_type) {
            let status = antigravity_tool_status(step, content);
            if self.replace_pending_tool(step, content, status.clone(), msg_store) {
                return;
            }

            add_normalized_entry(
                msg_store,
                &self.entry_index,
                NormalizedEntry {
                    timestamp: step.created_at.clone(),
                    entry_type: NormalizedEntryType::ToolUse {
                        tool_name: step.step_type.to_ascii_lowercase(),
                        action_type: ActionType::Tool {
                            tool_name: step.step_type.to_ascii_lowercase(),
                            arguments: None,
                            result: Some(ToolResult::markdown(content.to_string())),
                        },
                        status,
                    },
                    content: step.step_type.clone(),
                    metadata: serde_json::to_value(step).ok(),
                },
            );
            return;
        }

        add_normalized_entry(
            msg_store,
            &self.entry_index,
            NormalizedEntry {
                timestamp: step.created_at.clone(),
                entry_type: NormalizedEntryType::SystemMessage,
                content: content.to_string(),
                metadata: serde_json::to_value(step).ok(),
            },
        );
    }

    fn replace_pending_tool(
        &mut self,
        step: &AntigravityTranscriptStep,
        result_text: &str,
        status: ToolStatus,
        msg_store: &Arc<MsgStore>,
    ) -> bool {
        let position = self
            .pending_tools
            .iter()
            .position(|tool| tool.expected_step_type == step.step_type)
            .or_else(|| (step.step_type == "ERROR_MESSAGE").then_some(0));
        let Some(position) = position else {
            return false;
        };
        let Some(info) = self.pending_tools.remove(position) else {
            return false;
        };
        let entry = NormalizedEntry {
            timestamp: step.created_at.clone(),
            entry_type: NormalizedEntryType::ToolUse {
                tool_name: info.tool_name,
                action_type: action_type_with_antigravity_result(
                    info.action_type,
                    result_text.to_string(),
                    status.clone(),
                ),
                status,
            },
            content: info.content,
            metadata: serde_json::to_value(step).ok(),
        };
        replace_normalized_entry(msg_store, info.entry_index, entry);
        true
    }

    fn add_token_usage(&mut self, msg_store: &Arc<MsgStore>) {
        if self.token_usage_added {
            return;
        }
        self.token_usage_added = true;

        let output_tokens =
            estimate_tokens_from_chars(self.output_chars.try_into().unwrap_or(u32::MAX));
        let total_tokens = self
            .prompt_tokens
            .load(Ordering::Relaxed)
            .saturating_add(output_tokens);
        if total_tokens > 0 {
            add_token_usage_entry(
                msg_store,
                &self.entry_index,
                total_tokens,
                self.context_window,
            );
        }
    }

    fn has_output(&self) -> bool {
        self.output_chars > 0 || self.assistant_message_added
    }
}

async fn tail_antigravity_transcript(
    msg_store: Arc<MsgStore>,
    entry_index: EntryIndexProvider,
    prompt_tokens: Arc<AtomicU32>,
    context_window: u32,
    transcript_path: PathBuf,
    worktree_path: PathBuf,
    transcript_assistant_message_seen: Arc<AtomicBool>,
    current_turn_started_at: DateTime<Utc>,
) {
    let mut state = AntigravityTranscriptState::new(
        entry_index,
        prompt_tokens,
        context_window,
        worktree_path,
        transcript_assistant_message_seen,
        current_turn_started_at,
    );
    let mut processed_steps = HashSet::new();
    let mut msg_stream = msg_store.history_plus_stream();

    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(TRANSCRIPT_POLL_INTERVAL_MS)) => {
                read_new_transcript_steps(&transcript_path, &mut processed_steps, &mut state, &msg_store).await;
            }
            maybe_msg = msg_stream.next() => {
                match maybe_msg {
                    Some(Ok(LogMsg::Finished)) | None => {
                        drain_transcript_after_finish(
                            &transcript_path,
                            &mut processed_steps,
                            &mut state,
                            &msg_store,
                        )
                        .await;
                        if state.has_output() {
                            state.add_token_usage(&msg_store);
                        }
                        return;
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn drain_transcript_after_finish(
    transcript_path: &Path,
    processed_steps: &mut HashSet<u64>,
    state: &mut AntigravityTranscriptState,
    msg_store: &Arc<MsgStore>,
) {
    read_new_transcript_steps(transcript_path, processed_steps, state, msg_store).await;

    for _ in 0..TRANSCRIPT_FINISH_GRACE_POLLS {
        tokio::time::sleep(Duration::from_millis(TRANSCRIPT_POLL_INTERVAL_MS)).await;
        read_new_transcript_steps(transcript_path, processed_steps, state, msg_store).await;
    }
}

async fn read_new_transcript_steps(
    transcript_path: &Path,
    processed_steps: &mut HashSet<u64>,
    state: &mut AntigravityTranscriptState,
    msg_store: &Arc<MsgStore>,
) {
    let Ok(contents) = tokio::fs::read_to_string(transcript_path).await else {
        return;
    };

    for (line_index, line) in contents.lines().enumerate() {
        let Ok(step) = serde_json::from_str::<AntigravityTranscriptStep>(line) else {
            continue;
        };
        let step_key = step.step_index.unwrap_or(line_index as u64);
        if processed_steps.insert(step_key) {
            state.handle_step(step, msg_store);
        }
    }
}

fn antigravity_transcript_path(antigravity_home: &Path, session_id: &str) -> PathBuf {
    antigravity_home
        .join("brain")
        .join(session_id)
        .join(".system_generated")
        .join("logs")
        .join("transcript.jsonl")
}

fn action_type_for_antigravity_tool(
    tool_name: &str,
    args: &Value,
    worktree_path: &str,
) -> ActionType {
    match tool_name {
        "run_command" | "command" => {
            let command =
                antigravity_arg_string(args, &["CommandLine", "command"]).unwrap_or_default();
            ActionType::CommandRun {
                category: CommandCategory::from_command(&command),
                command,
                result: None,
            }
        }
        "view_file" | "read_file" => ActionType::FileRead {
            path: relative_antigravity_path(
                antigravity_arg_string(args, &["AbsolutePath", "path", "file_path"]).as_deref(),
                worktree_path,
            ),
        },
        "list_dir" | "list_directory" => ActionType::FileRead {
            path: relative_antigravity_path(
                antigravity_arg_string(args, &["DirectoryPath", "path"]).as_deref(),
                worktree_path,
            ),
        },
        "write_to_file" | "write_file" => ActionType::FileEdit {
            path: relative_antigravity_path(
                antigravity_arg_string(args, &["TargetFile", "path", "file_path"]).as_deref(),
                worktree_path,
            ),
            changes: vec![FileChange::Write {
                content: antigravity_arg_string(args, &["CodeContent", "content"])
                    .unwrap_or_default(),
            }],
        },
        "grep_search" | "search" => ActionType::Search {
            query: antigravity_arg_string(args, &["Query", "Pattern", "query", "pattern"])
                .unwrap_or_default(),
        },
        "read_url" | "execute_url" | "web_fetch" => ActionType::WebFetch {
            url: antigravity_arg_string(args, &["Url", "URL", "url"]).unwrap_or_default(),
        },
        _ => ActionType::Tool {
            tool_name: tool_name.to_string(),
            arguments: Some(args.clone()),
            result: None,
        },
    }
}

fn action_type_with_antigravity_result(
    action_type: ActionType,
    result_text: String,
    status: ToolStatus,
) -> ActionType {
    match action_type {
        ActionType::CommandRun {
            command, category, ..
        } => ActionType::CommandRun {
            command,
            category,
            result: Some(CommandRunResult {
                exit_status: None,
                output: Some(result_text),
            }),
        },
        ActionType::Tool {
            tool_name,
            arguments,
            ..
        } => ActionType::Tool {
            tool_name,
            arguments,
            result: Some(ToolResult::markdown(result_text)),
        },
        other if matches!(status, ToolStatus::Failed) => ActionType::Tool {
            tool_name: "antigravity_tool".to_string(),
            arguments: None,
            result: Some(ToolResult::markdown(result_text)),
        },
        other => other,
    }
}

fn antigravity_tool_content(tool_name: &str, args: &Value, action_type: &ActionType) -> String {
    if let Some(action) =
        antigravity_arg_string(args, &["toolAction", "toolSummary", "Description"])
    {
        return action;
    }

    match action_type {
        ActionType::CommandRun { command, .. } if !command.is_empty() => {
            format!("Run command: {command}")
        }
        ActionType::FileRead { path } if !path.is_empty() => format!("Read {path}"),
        ActionType::FileEdit { path, .. } if !path.is_empty() => format!("Edit {path}"),
        ActionType::Search { query } if !query.is_empty() => format!("Search for {query}"),
        ActionType::WebFetch { url } if !url.is_empty() => format!("Fetch {url}"),
        _ => tool_name.to_string(),
    }
}

fn expected_step_type_for_tool(tool_name: &str) -> String {
    match tool_name {
        "run_command" | "command" => "RUN_COMMAND",
        "view_file" | "read_file" => "VIEW_FILE",
        "list_dir" | "list_directory" => "LIST_DIRECTORY",
        "write_to_file" | "write_file" => "CODE_ACTION",
        "grep_search" | "search" => "GREP_SEARCH",
        "generate_image" => "GENERATE_IMAGE",
        "schedule" => "GENERIC",
        _ => "GENERIC",
    }
    .to_string()
}

fn is_antigravity_tool_result_step(step_type: &str) -> bool {
    matches!(
        step_type,
        "RUN_COMMAND"
            | "LIST_DIRECTORY"
            | "VIEW_FILE"
            | "CODE_ACTION"
            | "GREP_SEARCH"
            | "GENERATE_IMAGE"
            | "GENERIC"
    )
}

fn antigravity_tool_status(step: &AntigravityTranscriptStep, content: &str) -> ToolStatus {
    if !matches!(step.status.as_deref(), Some("DONE")) {
        return ToolStatus::Created;
    }
    if content.contains("Tool is running as a background task") {
        return ToolStatus::Created;
    }
    if content.contains("failed with exit code") || content.contains("Error invalid tool call") {
        return ToolStatus::Failed;
    }
    ToolStatus::Success
}

fn antigravity_arg_string(args: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        args.get(*name)
            .and_then(decode_antigravity_arg)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn decode_antigravity_arg(value: &Value) -> Option<String> {
    match value {
        Value::String(raw) => serde_json::from_str::<String>(raw)
            .ok()
            .or_else(|| Some(raw.clone())),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn relative_antigravity_path(path: Option<&str>, worktree_path: &str) -> String {
    let Some(path) = path else {
        return String::new();
    };
    let path = path.strip_prefix("file://").unwrap_or(path);
    make_path_relative(path, worktree_path)
}

fn add_token_usage_entry(
    msg_store: &Arc<MsgStore>,
    entry_index: &EntryIndexProvider,
    total_tokens: u32,
    context_window: u32,
) {
    add_normalized_entry(
        msg_store,
        entry_index,
        NormalizedEntry {
            timestamp: None,
            entry_type: NormalizedEntryType::TokenUsageInfo(TokenUsageInfo {
                total_tokens,
                model_context_window: context_window,
            }),
            content: format!(
                "Estimated tokens used: {total_tokens} / Context window: {context_window}"
            ),
            metadata: None,
        },
    );
}

async fn discover_antigravity_model_lines(cmd: &CmdOverrides, current_dir: &Path) -> Vec<String> {
    let command_parts = match antigravity_discovery_command(cmd)
        .and_then(|builder| builder.build_follow_up(&["models".to_string()]))
    {
        Ok(parts) => parts,
        Err(error) => {
            tracing::warn!("Failed to build Antigravity model discovery command: {error}");
            return fallback_antigravity_model_lines();
        }
    };

    let (program_path, args) = match command_parts.into_resolved().await {
        Ok(parts) => parts,
        Err(error) => {
            tracing::warn!("Failed to resolve Antigravity model discovery command: {error}");
            return fallback_antigravity_model_lines();
        }
    };

    let mut command = Command::new(program_path);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(current_dir)
        .args(args);

    if let Some(env_vars) = &cmd.env {
        command.envs(env_vars);
    }

    match tokio::time::timeout(
        Duration::from_secs(ANTIGRAVITY_MODEL_DISCOVERY_TIMEOUT_SECS),
        command.output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => {
            let lines = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            if lines.is_empty() {
                fallback_antigravity_model_lines()
            } else {
                lines
            }
        }
        Ok(Ok(output)) => {
            tracing::warn!(
                "Antigravity model discovery failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            fallback_antigravity_model_lines()
        }
        Ok(Err(error)) => {
            tracing::warn!("Antigravity model discovery failed: {error}");
            fallback_antigravity_model_lines()
        }
        Err(_) => {
            tracing::warn!(
                "Antigravity model discovery timed out after {ANTIGRAVITY_MODEL_DISCOVERY_TIMEOUT_SECS}s"
            );
            fallback_antigravity_model_lines()
        }
    }
}

fn antigravity_discovery_command(cmd: &CmdOverrides) -> Result<CommandBuilder, CommandBuildError> {
    let builder = CommandBuilder::new("agy");
    Ok(if let Some(base) = &cmd.base_command_override {
        builder.override_base(base.clone())
    } else {
        builder
    })
}

fn fallback_antigravity_model_lines() -> Vec<String> {
    FALLBACK_ANTIGRAVITY_MODELS
        .iter()
        .map(|line| (*line).to_string())
        .collect()
}

fn antigravity_model_selector_config(lines: &[String]) -> ModelSelectorConfig {
    let mut models = Vec::<ModelInfo>::new();

    for line in lines {
        let (base_model, effort) = split_model_effort(line);
        if base_model.is_empty() {
            continue;
        }

        if let Some(model) = models
            .iter_mut()
            .find(|model| model.id.eq_ignore_ascii_case(&base_model))
        {
            if let Some(effort) = effort {
                push_effort_option(&mut model.reasoning_options, &effort);
            }
            continue;
        }

        let mut reasoning_options = Vec::new();
        if let Some(effort) = effort {
            push_effort_option(&mut reasoning_options, &effort);
        }

        models.push(ModelInfo {
            id: base_model.clone(),
            name: base_model,
            provider_id: None,
            reasoning_options,
        });
    }

    if models.is_empty() {
        return antigravity_model_selector_config(&fallback_antigravity_model_lines());
    }

    ModelSelectorConfig {
        default_model: models.first().map(|model| model.id.clone()),
        models,
        permissions: vec![PermissionPolicy::Auto],
        ..Default::default()
    }
}

fn push_effort_option(options: &mut Vec<ReasoningOption>, effort: &str) {
    let id = normalize_effort_id(effort);
    if id.is_empty() || options.iter().any(|option| option.id == id) {
        return;
    }

    options.push(ReasoningOption {
        id,
        label: effort_label(effort),
        is_default: options.is_empty(),
    });
}

fn compose_antigravity_model_arg(model: Option<&str>, effort: Option<&str>) -> Option<String> {
    let model = model.map(str::trim).filter(|model| !model.is_empty())?;
    let (base_model, embedded_effort) = split_model_effort(model);
    let selected_effort = effort
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .map(effort_label)
        .or(embedded_effort);

    Some(match selected_effort {
        Some(effort) => format!("{base_model} ({effort})"),
        None => base_model,
    })
}

fn preset_model_and_reasoning(
    model: Option<&str>,
    effort: Option<&str>,
) -> (Option<String>, Option<String>) {
    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return (None, None);
    };

    let (base_model, embedded_effort) = split_model_effort(model);
    let reasoning_id = effort
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .map(normalize_effort_id)
        .or_else(|| embedded_effort.as_deref().map(normalize_effort_id));

    (Some(base_model), reasoning_id)
}

fn split_model_effort(value: &str) -> (String, Option<String>) {
    let trimmed = value.trim();
    if let Some(without_suffix) = trimmed.strip_suffix(')')
        && let Some(idx) = without_suffix.rfind(" (")
    {
        let base = without_suffix[..idx].trim();
        let effort = without_suffix[idx + 2..].trim();
        if !base.is_empty() && !effort.is_empty() {
            return (base.to_string(), Some(effort.to_string()));
        }
    }

    (trimmed.to_string(), None)
}

fn normalize_effort_id(effort: &str) -> String {
    effort.trim().to_ascii_lowercase().replace(' ', "-")
}

fn effort_label(effort: &str) -> String {
    match normalize_effort_id(effort).as_str() {
        "low" => "Low".to_string(),
        "medium" => "Medium".to_string(),
        "high" => "High".to_string(),
        "thinking" => "Thinking".to_string(),
        value => value
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn estimate_tokens(text: &str) -> u32 {
    estimate_tokens_from_chars(text.chars().count().try_into().unwrap_or(u32::MAX))
}

fn estimate_tokens_from_chars(chars: u32) -> u32 {
    if chars == 0 { 0 } else { chars.div_ceil(4) }
}

fn is_uuid_like(value: &str) -> bool {
    value.len() == 36 && value.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-')
}

fn antigravity_temp_log_path() -> Result<PathBuf, ExecutorError> {
    let dir = env::temp_dir().join("vibe-kanban-agy");
    fs::create_dir_all(&dir).map_err(ExecutorError::Io)?;
    Ok(dir.join(format!("{}.log", Uuid::new_v4())))
}

fn non_empty_env_path(cmd: &CmdOverrides, key: &str) -> Option<PathBuf> {
    cmd.env
        .as_ref()
        .and_then(|vars| vars.get(key))
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var(key)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
        })
}

fn antigravity_cli_home(cmd: &CmdOverrides) -> Option<PathBuf> {
    non_empty_env_path(cmd, "ANTIGRAVITY_CLI_HOME")
        .or_else(|| dirs::home_dir().map(|home| home.join(".gemini").join("antigravity-cli")))
}

fn antigravity_config_dir(cmd: &CmdOverrides) -> Option<PathBuf> {
    non_empty_env_path(cmd, "ANTIGRAVITY_CONFIG_DIR")
        .or_else(|| dirs::home_dir().map(|home| home.join(".gemini").join("config")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use workspace_utils::log_msg::LogMsg;

    use crate::logs::utils::patch::extract_normalized_entry_from_patch;

    fn latest_normalized_entries(msg_store: &MsgStore) -> Vec<NormalizedEntry> {
        let mut entries = BTreeMap::new();
        for msg in msg_store.get_history() {
            if let LogMsg::JsonPatch(patch) = msg
                && let Some((index, entry)) = extract_normalized_entry_from_patch(&patch)
            {
                entries.insert(index, entry);
            }
        }
        entries.into_values().collect()
    }

    #[test]
    fn detects_wrapper_conversation_marker() {
        assert!(is_uuid_like("3df9e047-12aa-4101-93e3-8a30ab0cbe43"));
        assert!(!is_uuid_like("not-a-session"));
    }

    #[test]
    fn estimates_tokens_from_character_count() {
        assert_eq!(estimate_tokens_from_chars(0), 0);
        assert_eq!(estimate_tokens_from_chars(1), 1);
        assert_eq!(estimate_tokens_from_chars(4), 1);
        assert_eq!(estimate_tokens_from_chars(5), 2);
    }

    #[test]
    fn groups_model_efforts_for_selector() {
        let lines = fallback_antigravity_model_lines();
        let config = antigravity_model_selector_config(&lines);

        let flash = config
            .models
            .iter()
            .find(|model| model.id == "Gemini 3.5 Flash")
            .expect("flash model");
        assert_eq!(config.default_model.as_deref(), Some("Gemini 3.5 Flash"));
        assert_eq!(
            flash
                .reasoning_options
                .iter()
                .map(|option| (option.id.as_str(), option.label.as_str(), option.is_default))
                .collect::<Vec<_>>(),
            vec![
                ("medium", "Medium", true),
                ("high", "High", false),
                ("low", "Low", false),
            ]
        );
    }

    #[test]
    fn composes_cli_model_from_base_and_effort() {
        assert_eq!(
            compose_antigravity_model_arg(Some("Gemini 3.5 Flash"), Some("low")).as_deref(),
            Some("Gemini 3.5 Flash (Low)")
        );
        assert_eq!(
            compose_antigravity_model_arg(Some("Gemini 3.5 Flash (Medium)"), Some("high"))
                .as_deref(),
            Some("Gemini 3.5 Flash (High)")
        );
    }

    #[test]
    fn parses_full_model_for_preset_options() {
        let (model_id, reasoning_id) =
            preset_model_and_reasoning(Some("Claude Sonnet 4.6 (Thinking)"), None);

        assert_eq!(model_id.as_deref(), Some("Claude Sonnet 4.6"));
        assert_eq!(reasoning_id.as_deref(), Some("thinking"));
    }

    #[test]
    fn decodes_antigravity_quoted_args() {
        let args = serde_json::json!({
            "CommandLine": "\"pwd\"",
            "WaitMsBeforeAsync": "500"
        });

        assert_eq!(
            antigravity_arg_string(&args, &["CommandLine"]).as_deref(),
            Some("pwd")
        );
        assert_eq!(
            antigravity_arg_string(&args, &["WaitMsBeforeAsync"]).as_deref(),
            Some("500")
        );
    }

    #[test]
    fn transcript_steps_create_thinking_tool_result_and_assistant_entries() {
        let msg_store = Arc::new(MsgStore::new());
        let assistant_seen = Arc::new(AtomicBool::new(false));
        let mut state = AntigravityTranscriptState::new(
            EntryIndexProvider::test_new(),
            Arc::new(AtomicU32::new(0)),
            DEFAULT_ANTIGRAVITY_CONTEXT_WINDOW,
            PathBuf::from("/repo"),
            assistant_seen,
            DateTime::parse_from_rfc3339("2026-06-13T23:59:59Z")
                .unwrap()
                .with_timezone(&Utc),
        );

        let planner = serde_json::from_value::<AntigravityTranscriptStep>(serde_json::json!({
            "step_index": 1,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "created_at": "2026-06-14T00:00:00Z",
            "thinking": "Need to inspect the workspace.",
            "tool_calls": [{
                "name": "run_command",
                "args": {
                    "CommandLine": "\"pwd\"",
                    "Cwd": "\"/repo\"",
                    "toolAction": "\"Running pwd\""
                }
            }]
        }))
        .unwrap();
        state.handle_step(planner, &msg_store);

        let result = serde_json::from_value::<AntigravityTranscriptStep>(serde_json::json!({
            "step_index": 2,
            "source": "MODEL",
            "type": "RUN_COMMAND",
            "status": "DONE",
            "created_at": "2026-06-14T00:00:01Z",
            "content": "Created At: 2026-06-14T00:00:01Z\nCompleted At: 2026-06-14T00:00:01Z\n\nThe command completed successfully.\nOutput:\n/repo\n"
        }))
        .unwrap();
        state.handle_step(result, &msg_store);

        let assistant = serde_json::from_value::<AntigravityTranscriptStep>(serde_json::json!({
            "step_index": 3,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "created_at": "2026-06-14T00:00:02Z",
            "content": "The workspace is /repo."
        }))
        .unwrap();
        state.handle_step(assistant, &msg_store);

        let follow_up = serde_json::from_value::<AntigravityTranscriptStep>(serde_json::json!({
            "step_index": 4,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "created_at": "2026-06-14T00:00:03Z",
            "content": "Done."
        }))
        .unwrap();
        state.handle_step(follow_up, &msg_store);

        let entries = latest_normalized_entries(&msg_store);
        assert!(matches!(
            entries.first().map(|entry| &entry.entry_type),
            Some(NormalizedEntryType::Thinking)
        ));

        let tool = entries
            .iter()
            .find(|entry| matches!(entry.entry_type, NormalizedEntryType::ToolUse { .. }))
            .expect("tool entry");
        match &tool.entry_type {
            NormalizedEntryType::ToolUse {
                tool_name,
                action_type:
                    ActionType::CommandRun {
                        command, result, ..
                    },
                status,
            } => {
                assert_eq!(tool_name, "run_command");
                assert_eq!(command, "pwd");
                assert!(matches!(status, ToolStatus::Success));
                assert!(
                    result
                        .as_ref()
                        .and_then(|result| result.output.as_deref())
                        .is_some_and(|output| output.contains("/repo"))
                );
            }
            other => panic!("unexpected tool entry: {other:?}"),
        }

        let assistant_messages = entries
            .iter()
            .filter(|entry| matches!(entry.entry_type, NormalizedEntryType::AssistantMessage))
            .map(|entry| entry.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(assistant_messages, vec!["The workspace is /repo.", "Done."]);
    }

    #[test]
    fn transcript_grep_search_result_replaces_pending_tool() {
        let msg_store = Arc::new(MsgStore::new());
        let mut state = AntigravityTranscriptState::new(
            EntryIndexProvider::test_new(),
            Arc::new(AtomicU32::new(0)),
            DEFAULT_ANTIGRAVITY_CONTEXT_WINDOW,
            PathBuf::from("/repo"),
            Arc::new(AtomicBool::new(false)),
            DateTime::parse_from_rfc3339("2026-06-13T23:59:59Z")
                .unwrap()
                .with_timezone(&Utc),
        );

        let planner = serde_json::from_value::<AntigravityTranscriptStep>(serde_json::json!({
            "step_index": 1,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "tool_calls": [{
                "name": "grep_search",
                "args": {
                    "Query": "\"needle\"",
                    "SearchPath": "\"/repo\"",
                    "toolAction": "\"Searching for needle\""
                }
            }]
        }))
        .unwrap();
        state.handle_step(planner, &msg_store);

        let result = serde_json::from_value::<AntigravityTranscriptStep>(serde_json::json!({
            "step_index": 2,
            "source": "MODEL",
            "type": "GREP_SEARCH",
            "status": "DONE",
            "content": "marker.txt:2:beta needle"
        }))
        .unwrap();
        state.handle_step(result, &msg_store);

        let entries = latest_normalized_entries(&msg_store);
        let tool_entries = entries
            .iter()
            .filter(|entry| matches!(entry.entry_type, NormalizedEntryType::ToolUse { .. }))
            .collect::<Vec<_>>();
        assert_eq!(tool_entries.len(), 1);

        match &tool_entries[0].entry_type {
            NormalizedEntryType::ToolUse {
                tool_name,
                action_type: ActionType::Search { query },
                status,
            } => {
                assert_eq!(tool_name, "grep_search");
                assert_eq!(query, "needle");
                assert!(matches!(status, ToolStatus::Success));
            }
            other => panic!("unexpected search tool entry: {other:?}"),
        }
    }

    #[test]
    fn transcript_skips_previous_turn_steps_on_resume() {
        let msg_store = Arc::new(MsgStore::new());
        let mut state = AntigravityTranscriptState::new(
            EntryIndexProvider::test_new(),
            Arc::new(AtomicU32::new(0)),
            DEFAULT_ANTIGRAVITY_CONTEXT_WINDOW,
            PathBuf::from("/repo"),
            Arc::new(AtomicBool::new(false)),
            DateTime::parse_from_rfc3339("2026-06-14T00:01:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );

        let previous_turn = serde_json::from_value::<AntigravityTranscriptStep>(serde_json::json!({
            "step_index": 1,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "created_at": "2026-06-14T00:00:00Z",
            "content": "old answer"
        }))
        .unwrap();
        state.handle_step(previous_turn, &msg_store);

        let current_turn = serde_json::from_value::<AntigravityTranscriptStep>(serde_json::json!({
            "step_index": 2,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "created_at": "2026-06-14T00:01:01Z",
            "content": "new answer"
        }))
        .unwrap();
        state.handle_step(current_turn, &msg_store);

        let assistant_messages = latest_normalized_entries(&msg_store)
            .iter()
            .filter(|entry| matches!(entry.entry_type, NormalizedEntryType::AssistantMessage))
            .map(|entry| entry.content.clone())
            .collect::<Vec<_>>();
        assert_eq!(assistant_messages, vec!["new answer"]);
    }
}
