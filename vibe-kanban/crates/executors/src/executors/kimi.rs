use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use async_trait::async_trait;
use derivative::Derivative;
use futures::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{io::AsyncWriteExt, process::Command};
use ts_rs::TS;
use workspace_utils::{
    command_ext::GroupSpawnNoWindowExt, msg_store::MsgStore, path::make_path_relative,
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
            ConversationPatch, EntryIndexProvider,
            patch::{self, add_normalized_entry, replace_normalized_entry},
            shell_command_parsing::CommandCategory,
        },
    },
    model_selector::{ModelInfo, ModelSelectorConfig, PermissionPolicy},
    profile::ExecutorConfig,
};

const DEFAULT_KIMI_CONTEXT_WINDOW: u32 = 262_144;

#[derive(Derivative, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[derivative(Debug, PartialEq)]
pub struct KimiCode {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        title = "Model",
        description = "Kimi model ID passed to `kimi --model`."
    )]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        title = "YOLO",
        description = "Pass `--yolo` to Kimi. Print mode already auto-approves tool calls."
    )]
    pub yolo: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        title = "Context Window",
        description = "Model context window used for Vibe Kanban's context usage gauge. If unset, Kimi config is read and then 262144 is used as a fallback."
    )]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        title = "MCP Config File",
        description = "Path passed to `kimi --mcp-config-file`."
    )]
    pub mcp_config_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        title = "MCP Config JSON",
        description = "Inline JSON passed to `kimi --mcp-config`."
    )]
    pub mcp_config: Option<String>,
    #[serde(flatten)]
    pub cmd: CmdOverrides,
}

impl KimiCode {
    fn build_command_builder(&self) -> Result<CommandBuilder, CommandBuildError> {
        let mut builder =
            CommandBuilder::new("kimi").params(["--print", "--output-format", "stream-json"]);

        if let Some(model) = &self.model {
            builder = builder.extend_params(["--model", model.as_str()]);
        }

        if self.yolo.unwrap_or(true) {
            builder = builder.extend_params(["--yolo"]);
        }

        if let Some(path) = &self.mcp_config_file {
            builder = builder.extend_params(["--mcp-config-file", path]);
        }

        if let Some(config) = &self.mcp_config {
            builder = builder.extend_params(["--mcp-config", config]);
        }

        apply_overrides(builder, &self.cmd)
    }

    async fn spawn_internal(
        &self,
        current_dir: &Path,
        prompt: &str,
        command_parts: CommandParts,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let (program_path, args) = command_parts.into_resolved().await?;
        let combined_prompt = self.append_prompt.combine_prompt(prompt);

        let mut command = Command::new(program_path);
        command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(current_dir)
            .args(&args);

        env.clone()
            .with_profile(&self.cmd)
            .apply_to_command(&mut command);

        let mut child = command.group_spawn_no_window()?;

        if let Some(mut stdin) = child.inner().stdin.take() {
            stdin.write_all(combined_prompt.as_bytes()).await?;
            stdin.shutdown().await?;
        }

        Ok(child.into())
    }

    fn share_dir(&self) -> Option<PathBuf> {
        kimi_share_dir(&self.cmd)
    }

    fn effective_context_window(&self) -> u32 {
        self.context_window
            .filter(|value| *value > 0)
            .or_else(|| context_window_from_config(self.model.as_deref(), &self.cmd))
            .unwrap_or(DEFAULT_KIMI_CONTEXT_WINDOW)
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for KimiCode {
    fn apply_overrides(&mut self, executor_config: &ExecutorConfig) {
        if let Some(model_id) = &executor_config.model_id {
            self.model = Some(model_id.clone());
        }
        if let Some(permission_policy) = executor_config.permission_policy.clone() {
            self.yolo = Some(matches!(permission_policy, PermissionPolicy::Auto));
        }
    }

    async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let command_parts = self.build_command_builder()?.build_initial()?;
        self.spawn_internal(current_dir, prompt, command_parts, env)
            .await
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        _reset_to_message_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let args = ["--resume".to_string(), session_id.to_string()];
        let command_parts = self.build_command_builder()?.build_follow_up(&args)?;
        self.spawn_internal(current_dir, prompt, command_parts, env)
            .await
    }

    fn normalize_logs(
        &self,
        msg_store: Arc<MsgStore>,
        worktree_path: &Path,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        normalize_logs(
            msg_store,
            worktree_path,
            self.share_dir(),
            self.effective_context_window(),
        )
    }

    fn default_mcp_config_path(&self) -> Option<PathBuf> {
        self.share_dir().map(|home| home.join("mcp.json"))
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        let binary_found =
            workspace_utils::shell::resolve_executable_path_blocking("kimi").is_some();
        if !binary_found {
            return AvailabilityInfo::NotFound;
        }

        let Some(share_dir) = self.share_dir() else {
            return AvailabilityInfo::InstallationFound;
        };

        if let Some(timestamp) =
            std::fs::metadata(share_dir.join("credentials").join("kimi-code.json"))
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
        {
            return AvailabilityInfo::LoginDetected {
                last_auth_timestamp: timestamp,
            };
        }

        let mcp_config_found = self
            .default_mcp_config_path()
            .map(|p| p.exists())
            .unwrap_or(false);
        let config_found = share_dir.join("config.toml").exists();

        if mcp_config_found || config_found {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }

    fn get_preset_options(&self) -> ExecutorConfig {
        ExecutorConfig {
            executor: BaseCodingAgent::KimiCode,
            variant: None,
            model_id: self
                .model
                .clone()
                .or_else(|| Some("kimi-code/kimi-for-coding".to_string())),
            agent_id: None,
            reasoning_id: None,
            permission_policy: Some(PermissionPolicy::Auto),
        }
    }

    async fn discover_options(
        &self,
        _workdir: Option<&Path>,
        _repo_path: Option<&Path>,
    ) -> Result<futures::stream::BoxStream<'static, json_patch::Patch>, ExecutorError> {
        let options = ExecutorDiscoveredOptions {
            model_selector: ModelSelectorConfig {
                models: vec![ModelInfo {
                    id: "kimi-code/kimi-for-coding".to_string(),
                    name: "Kimi K2.6".to_string(),
                    provider_id: None,
                    reasoning_options: vec![],
                }],
                default_model: Some("kimi-code/kimi-for-coding".to_string()),
                permissions: vec![PermissionPolicy::Auto],
                ..Default::default()
            },
            ..Default::default()
        };
        Ok(Box::pin(futures::stream::once(async move {
            patch::executor_discovered_options(options)
        })))
    }
}

fn normalize_logs(
    msg_store: Arc<MsgStore>,
    worktree_path: &Path,
    share_dir: Option<PathBuf>,
    context_window: u32,
) -> Vec<tokio::task::JoinHandle<()>> {
    let entry_index = EntryIndexProvider::start_from(&msg_store);
    let stdout_handle = normalize_stdout_logs(
        msg_store.clone(),
        worktree_path.to_path_buf(),
        entry_index.clone(),
        context_window,
    );
    let stderr_handle = normalize_stderr_logs(msg_store, entry_index, share_dir, context_window);
    vec![stdout_handle, stderr_handle]
}

fn normalize_stdout_logs(
    msg_store: Arc<MsgStore>,
    worktree_path: PathBuf,
    entry_index: EntryIndexProvider,
    context_window: u32,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut state = KimiLogState::new(entry_index, context_window);
        let worktree_path = worktree_path.to_string_lossy().to_string();
        let mut stdout = msg_store.stdout_lines_stream();

        while let Some(Ok(line)) = stdout.next().await {
            state.handle_stdout_line(&line, &msg_store, &worktree_path);
        }
    })
}

fn normalize_stderr_logs(
    msg_store: Arc<MsgStore>,
    entry_index: EntryIndexProvider,
    share_dir: Option<PathBuf>,
    context_window: u32,
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
                    &share_dir,
                    context_window,
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
                &share_dir,
                context_window,
                &mut stored_session_id,
            );
        }
    })
}

fn handle_stderr_line(
    line: &str,
    msg_store: &Arc<MsgStore>,
    entry_index: &EntryIndexProvider,
    share_dir: &Option<PathBuf>,
    context_window: u32,
    stored_session_id: &mut bool,
) {
    if let Some(session_id) = extract_resume_session_id(line) {
        if !*stored_session_id {
            msg_store.push_session_id(session_id.clone());
            *stored_session_id = true;
        }
        if let Some(total_tokens) = latest_usage_from_session_file(share_dir, &session_id) {
            add_token_usage_entry(msg_store, entry_index, total_tokens, context_window);
        }
        return;
    }

    let trimmed = line.trim();
    if trimmed.is_empty() {
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

#[derive(Debug, Clone)]
struct KimiToolInfo {
    entry_index: usize,
    tool_name: String,
    action_type: ActionType,
    content: String,
}

struct KimiLogState {
    entry_index: EntryIndexProvider,
    tool_map: HashMap<String, KimiToolInfo>,
    context_window: u32,
}

impl KimiLogState {
    fn new(entry_index: EntryIndexProvider, context_window: u32) -> Self {
        Self {
            entry_index,
            tool_map: HashMap::new(),
            context_window,
        }
    }

    fn handle_stdout_line(&mut self, line: &str, msg_store: &Arc<MsgStore>, worktree_path: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }

        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            self.add_system_message(msg_store, trimmed.to_string(), None);
            return;
        };

        match value.get("role").and_then(Value::as_str) {
            Some("_usage") => {
                if let Some(token_count) = value
                    .get("token_count")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                {
                    add_token_usage_entry(
                        msg_store,
                        &self.entry_index,
                        token_count,
                        self.context_window,
                    );
                }
            }
            Some("assistant") => {
                self.handle_assistant_message(&value, msg_store, worktree_path);
            }
            Some("tool") => {
                self.handle_tool_result(&value, msg_store);
            }
            Some("user") => {}
            Some(role) if role.starts_with('_') => {}
            _ => self.add_system_message(msg_store, trimmed.to_string(), Some(value)),
        }
    }

    fn handle_assistant_message(
        &mut self,
        value: &Value,
        msg_store: &Arc<MsgStore>,
        worktree_path: &str,
    ) {
        if let Some(items) = value.get("content").and_then(Value::as_array) {
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("think") => {
                        if let Some(content) = item.get("think").and_then(Value::as_str)
                            && !content.trim().is_empty()
                        {
                            self.add_entry(
                                msg_store,
                                NormalizedEntryType::Thinking,
                                content.to_string(),
                                Some(item.clone()),
                            );
                        }
                    }
                    Some("text") => {
                        if let Some(content) = item.get("text").and_then(Value::as_str)
                            && !content.trim().is_empty()
                        {
                            self.add_entry(
                                msg_store,
                                NormalizedEntryType::AssistantMessage,
                                content.to_string(),
                                Some(item.clone()),
                            );
                        }
                    }
                    _ => {}
                }
            }
        }

        if let Some(tool_calls) = value.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                self.handle_tool_call(tool_call, msg_store, worktree_path);
            }
        }
    }

    fn handle_tool_call(
        &mut self,
        tool_call: &Value,
        msg_store: &Arc<MsgStore>,
        worktree_path: &str,
    ) {
        let Some(id) = tool_call.get("id").and_then(Value::as_str) else {
            return;
        };
        let Some(function) = tool_call.get("function") else {
            return;
        };
        let tool_name = function
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let arguments = parse_tool_arguments(function.get("arguments"));
        let action_type = action_type_for_tool(&tool_name, arguments.as_ref(), worktree_path);
        let content = tool_content(&tool_name, arguments.as_ref(), &action_type);

        let entry = NormalizedEntry {
            timestamp: None,
            entry_type: NormalizedEntryType::ToolUse {
                tool_name: tool_name.clone(),
                action_type: action_type.clone(),
                status: ToolStatus::Created,
            },
            content: content.clone(),
            metadata: Some(tool_call.clone()),
        };
        let index = self.entry_index.next();
        msg_store.push_patch(ConversationPatch::add_normalized_entry(index, entry));
        self.tool_map.insert(
            id.to_string(),
            KimiToolInfo {
                entry_index: index,
                tool_name,
                action_type,
                content,
            },
        );
    }

    fn handle_tool_result(&mut self, value: &Value, msg_store: &Arc<MsgStore>) {
        let Some(tool_call_id) = value.get("tool_call_id").and_then(Value::as_str) else {
            return;
        };
        let Some(info) = self.tool_map.get(tool_call_id).cloned() else {
            return;
        };

        let result_text = value
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let action_type = action_type_with_result(info.action_type, result_text.clone());
        let entry = NormalizedEntry {
            timestamp: None,
            entry_type: NormalizedEntryType::ToolUse {
                tool_name: info.tool_name,
                action_type,
                status: ToolStatus::Success,
            },
            content: info.content,
            metadata: Some(value.clone()),
        };
        replace_normalized_entry(msg_store, info.entry_index, entry);
    }

    fn add_entry(
        &self,
        msg_store: &Arc<MsgStore>,
        entry_type: NormalizedEntryType,
        content: String,
        metadata: Option<Value>,
    ) {
        add_normalized_entry(
            msg_store,
            &self.entry_index,
            NormalizedEntry {
                timestamp: None,
                entry_type,
                content,
                metadata,
            },
        );
    }

    fn add_system_message(
        &self,
        msg_store: &Arc<MsgStore>,
        content: String,
        metadata: Option<Value>,
    ) {
        self.add_entry(
            msg_store,
            NormalizedEntryType::SystemMessage,
            content,
            metadata,
        );
    }
}

fn parse_tool_arguments(raw: Option<&Value>) -> Option<Value> {
    match raw {
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).ok(),
        Some(value) => Some(value.clone()),
        None => None,
    }
}

fn action_type_for_tool(
    tool_name: &str,
    arguments: Option<&Value>,
    worktree_path: &str,
) -> ActionType {
    let arg = |name: &str| {
        arguments
            .and_then(|args| args.get(name))
            .and_then(Value::as_str)
    };
    match tool_name {
        "Shell" | "Bash" => {
            let command = arg("command").unwrap_or_default().to_string();
            ActionType::CommandRun {
                category: CommandCategory::from_command(&command),
                command,
                result: None,
            }
        }
        "ReadFile" | "Read" => ActionType::FileRead {
            path: relative_path(arg("path").or_else(|| arg("file_path")), worktree_path),
        },
        "WriteFile" => ActionType::FileEdit {
            path: relative_path(arg("path").or_else(|| arg("file_path")), worktree_path),
            changes: vec![FileChange::Write {
                content: arg("content").unwrap_or_default().to_string(),
            }],
        },
        "StrReplaceFile" | "Edit" | "MultiEdit" => ActionType::FileEdit {
            path: relative_path(arg("path").or_else(|| arg("file_path")), worktree_path),
            changes: vec![FileChange::Edit {
                unified_diff: String::new(),
                has_line_numbers: false,
            }],
        },
        "Glob" | "Grep" => ActionType::Search {
            query: arg("pattern")
                .or_else(|| arg("query"))
                .unwrap_or_default()
                .to_string(),
        },
        "WebFetch" | "Fetch" => ActionType::WebFetch {
            url: arg("url").unwrap_or_default().to_string(),
        },
        _ => ActionType::Tool {
            tool_name: tool_name.to_string(),
            arguments: arguments.cloned(),
            result: None,
        },
    }
}

fn action_type_with_result(action_type: ActionType, result_text: String) -> ActionType {
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
        other => other,
    }
}

fn tool_content(tool_name: &str, arguments: Option<&Value>, action_type: &ActionType) -> String {
    match action_type {
        ActionType::CommandRun { command, .. } if !command.is_empty() => {
            format!("Run command: {command}")
        }
        ActionType::FileRead { path } if !path.is_empty() => format!("Read {path}"),
        ActionType::FileEdit { path, .. } if !path.is_empty() => format!("Edit {path}"),
        ActionType::Search { query } if !query.is_empty() => format!("Search for {query}"),
        ActionType::WebFetch { url } if !url.is_empty() => format!("Fetch {url}"),
        _ => arguments
            .map(|args| format!("{tool_name}: {args}"))
            .unwrap_or_else(|| tool_name.to_string()),
    }
}

fn relative_path(path: Option<&str>, worktree_path: &str) -> String {
    path.map(|path| make_path_relative(path, worktree_path))
        .unwrap_or_default()
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
            content: format!("Tokens used: {total_tokens} / Context window: {context_window}"),
            metadata: None,
        },
    );
}

fn extract_resume_session_id(line: &str) -> Option<String> {
    let marker = "kimi -r ";
    let start = line.find(marker)? + marker.len();
    let id = line[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_hexdigit() || *ch == '-')
        .collect::<String>();
    (!id.is_empty()).then_some(id)
}

fn latest_usage_from_session_file(share_dir: &Option<PathBuf>, session_id: &str) -> Option<u32> {
    let share_dir = share_dir.as_ref()?;
    let sessions_dir = share_dir.join("sessions");
    let entries = std::fs::read_dir(sessions_dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path().join(session_id).join("context.jsonl");
        if path.exists() {
            return latest_usage_from_context_file(&path);
        }
    }

    None
}

fn latest_usage_from_context_file(path: &Path) -> Option<u32> {
    let contents = std::fs::read_to_string(path).ok()?;
    contents
        .lines()
        .filter_map(|line| {
            let value = serde_json::from_str::<Value>(line).ok()?;
            (value.get("role").and_then(Value::as_str) == Some("_usage"))
                .then(|| value.get("token_count"))
                .flatten()
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
        })
        .last()
}

fn kimi_share_dir(cmd: &CmdOverrides) -> Option<PathBuf> {
    cmd.env
        .as_ref()
        .and_then(|vars| vars.get("KIMI_SHARE_DIR"))
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var("KIMI_SHARE_DIR")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| dirs::home_dir().map(|home| home.join(".kimi")))
}

fn context_window_from_config(model: Option<&str>, cmd: &CmdOverrides) -> Option<u32> {
    let config_path = kimi_share_dir(cmd)?.join("config.toml");
    let config = std::fs::read_to_string(config_path).ok()?;
    let value = toml::from_str::<toml::Value>(&config).ok()?;
    let model_id = model.or_else(|| value.get("default_model")?.as_str())?;
    let context_window = value
        .get("models")?
        .get(model_id)?
        .get("max_context_size")?
        .as_integer()?;
    u32::try_from(context_window)
        .ok()
        .filter(|value| *value > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_session_id_from_resume_footer() {
        assert_eq!(
            extract_resume_session_id(
                "To resume this session: kimi -r 89a0f9db-f962-44f5-8a6b-9e717198c927"
            )
            .as_deref(),
            Some("89a0f9db-f962-44f5-8a6b-9e717198c927")
        );
    }

    #[test]
    fn reads_latest_usage_from_context_jsonl() {
        let dir = std::env::temp_dir().join(format!("kimi-context-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("context.jsonl");
        std::fs::write(
            &path,
            r#"{"role":"_usage","token_count":10}"#.to_string()
                + "\n"
                + r#"{"role":"assistant","content":[]}"#
                + "\n"
                + r#"{"role":"_usage","token_count":42}"#
                + "\n",
        )
        .unwrap();

        assert_eq!(latest_usage_from_context_file(&path), Some(42));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn maps_write_file_tool_call() {
        let args = serde_json::json!({"path": "/repo/file.txt", "content": "hello"});
        let action = action_type_for_tool("WriteFile", Some(&args), "/repo");
        match action {
            ActionType::FileEdit { path, changes } => {
                assert_eq!(path, "file.txt");
                assert!(matches!(changes.first(), Some(FileChange::Write { .. })));
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }
}
