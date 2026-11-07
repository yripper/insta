//! The MCP tools. Each one is a thin wrapper over `Actions`; results are
//! pretty JSON text, and failures come back as tool errors Claude can read.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};

use super::instructions::current_instructions;
use super::state::AppState;
use crate::domain::actions::Actions;
use crate::domain::errors::{Error, Result as DResult};
use crate::ig::types::Micros;
use crate::store::{LabelKind, WatchLevel};

#[derive(Clone)]
pub struct InstaServer {
    state: Arc<AppState>,
    /// `~/.insta/style.md`; `None` in tests.
    style_path: Option<PathBuf>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ListArgs {
    /// Part of a username or name. Without it muted chats are hidden.
    pub query: Option<String>,
    /// Max chats (default 30).
    pub limit: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ChatArgs {
    /// Chat id, @username, or part of a name.
    pub chat: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct HistoryArgs {
    /// Chat id, @username, or part of a name.
    pub chat: String,
    /// How many recent messages (default 30).
    pub limit: Option<u32>,
    /// older_cursor from a previous read_history, for older messages.
    pub cursor: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct MarkArgs {
    pub chat: String,
    /// Mark seen up to this message id (default: the newest).
    pub item_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SendArgs {
    /// Chat id, @username, or part of a name. Must match exactly one.
    pub chat: String,
    /// The exact text the user approved.
    pub text: String,
    /// newest_ts the user has seen (from read_history). Needed to mark the chat seen.
    pub seen_until: Option<Micros>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SendStatusArgs {
    /// One job; omit for all recent jobs.
    pub job_id: Option<u64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct LabelArgs {
    pub chat: String,
    pub label: LabelKind,
    /// Up to 500 characters.
    pub note: Option<String>,
    /// For snoozed: RFC 3339 time with offset, e.g. 2026-11-08T09:00:00-03:00.
    pub snooze_until: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct WatchArgs {
    pub chat: String,
    pub level: WatchLevel,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct MyMessagesArgs {
    /// One chat. Give exactly one of chat or category.
    pub chat: Option<String>,
    /// A category slug.
    pub category: Option<String>,
    /// Max messages (default 60, max 200).
    pub limit: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ProfileArgs {
    /// Category slug, ^[a-z0-9_-]{1,32}$.
    pub category: String,
    /// 1 to 4000 characters describing the style.
    pub body: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct NotesArgs {
    pub chat: String,
    /// 1 to 2000 characters describing how the user writes in this chat.
    pub body: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CategoryArgs {
    pub chat: String,
    /// Category slug, ^[a-z0-9_-]{1,32}$, e.g. friends, family, work.
    pub category: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct TranscribeArgs {
    pub chat: String,
    /// Id of the voice note message.
    pub item_id: String,
}

#[derive(Serialize)]
struct Status {
    logged_in: bool,
    username: Option<String>,
    user_id: Option<String>,
    note: Option<String>,
    queued_sends: usize,
}

#[derive(Serialize)]
struct Categories {
    categories: Vec<String>,
}

fn json<T: Serialize>(v: &T) -> CallToolResult {
    match serde_json::to_string_pretty(v) {
        Ok(s) => CallToolResult::success(vec![ContentBlock::text(s)]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(format!("serialization failed: {e}"))]),
    }
}

pub fn parse_time(s: &str) -> DResult<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.timestamp())
        .map_err(|_| Error::Invalid(format!("'{s}' is not an RFC 3339 time like 2026-11-08T09:00:00-03:00")))
}

fn limit(v: Option<u32>, default: u32, max: u32) -> usize {
    v.unwrap_or(default).clamp(1, max) as usize
}

impl InstaServer {
    pub fn new(state: Arc<AppState>, style_path: Option<PathBuf>) -> InstaServer {
        InstaServer { state, style_path }
    }

    /// Runs one action against the live session, turning failures into tool
    /// errors and dropping the session when Instagram rejects it.
    async fn run<T, F, Fut>(&self, f: F) -> CallToolResult
    where
        T: Serialize,
        F: FnOnce(Arc<Actions>) -> Fut,
        Fut: Future<Output = DResult<T>>,
    {
        let outcome = match self.state.actions().await {
            Ok(actions) => f(actions).await,
            Err(e) => Err(e),
        };
        match outcome {
            Ok(v) => json(&v),
            Err(e) => {
                self.state.note_error(&e).await;
                // Error text can carry names: log only the kind.
                tracing::warn!(kind = e.kind(), "tool failed");
                CallToolResult::error(vec![ContentBlock::text(e.to_string())])
            }
        }
    }

    pub async fn call_pending(&self) -> CallToolResult {
        self.run(|a| async move { a.pending().await }).await
    }

    pub async fn call_send(&self, a: SendArgs) -> CallToolResult {
        self.run(|x| async move { x.send(&a.chat, &a.text, a.seen_until).await }).await
    }
}

#[tool_router(vis = "pub")]
impl InstaServer {
    #[tool(description = "Whether the Instagram session works, as whom, and how many sends are queued.")]
    async fn status(&self) -> Result<CallToolResult, McpError> {
        let queued_sends = self.state.queue().queued_count();
        let s = match self.state.actions().await {
            Ok(a) => Status {
                logged_in: true,
                username: Some(a.me().username.clone()),
                user_id: Some(a.me().user_id.clone()),
                note: None,
                queued_sends,
            },
            Err(e) => {
                Status { logged_in: false, username: None, user_id: None, note: Some(e.to_string()), queued_sends }
            }
        };
        Ok(json(&s))
    }

    #[tool(
        description = "Recent one-on-one chats, newest first, optionally filtered by username or name. Muted chats only show when searched."
    )]
    async fn list_chats(&self, Parameters(a): Parameters<ListArgs>) -> Result<CallToolResult, McpError> {
        let n = limit(a.limit, 30, 60);
        Ok(self.run(|x| async move { x.list_chats(a.query.as_deref(), n).await }).await)
    }

    #[tool(
        description = "One-on-one chats waiting on the user (the other person wrote last), newest first, with their latest messages. Muted, snoozed and done/waiting chats are left out. Marks nothing seen."
    )]
    async fn get_pending(&self) -> Result<CallToolResult, McpError> {
        Ok(self.call_pending().await)
    }

    #[tool(
        description = "Recent messages of a chat, oldest first. Returns newest_ts (pass as seen_until when replying) and older_cursor. Marks nothing seen."
    )]
    async fn read_history(&self, Parameters(a): Parameters<HistoryArgs>) -> Result<CallToolResult, McpError> {
        let n = limit(a.limit, 30, 100);
        Ok(self.run(|x| async move { x.read_history(&a.chat, n, a.cursor.as_deref()).await }).await)
    }

    #[tool(
        description = "Mark a chat seen on Instagram (the other person sees \"Seen\"), up to item_id or the newest message. Only when the user asks."
    )]
    async fn mark_read(&self, Parameters(a): Parameters<MarkArgs>) -> Result<CallToolResult, McpError> {
        Ok(self.run(|x| async move { x.mark_read(&a.chat, a.item_id.as_deref()).await }).await)
    }

    #[tool(
        description = "Queue a message as the user. ONLY after the user explicitly approved this exact text for this chat. Returns job_id, position and eta at once; messages go out 20-60 s apart. Check send_status. With seen_until, the chat is marked seen after sending unless new messages arrived."
    )]
    async fn send_message(&self, Parameters(a): Parameters<SendArgs>) -> Result<CallToolResult, McpError> {
        Ok(self.call_send(a).await)
    }

    #[tool(
        description = "Status of queued and recent sends: queued, sending, sent or failed, eta, sent_item_id, error, mark_error, unseen. A sent_item_id means it was sent: never resend."
    )]
    async fn send_status(&self, Parameters(a): Parameters<SendStatusArgs>) -> Result<CallToolResult, McpError> {
        Ok(self.run(|x| async move { x.send_status(a.job_id) }).await)
    }

    #[tool(
        description = "Label a chat: done (nothing owed) or waiting (ball in their court), both cleared by their next message; or snoozed until snooze_until. Hides it from get_pending."
    )]
    async fn set_label(&self, Parameters(a): Parameters<LabelArgs>) -> Result<CallToolResult, McpError> {
        Ok(self
            .run(|x| async move {
                let until = a.snooze_until.as_deref().map(parse_time).transpose()?;
                x.set_label(&a.chat, a.label, a.note, until).await
            })
            .await)
    }

    #[tool(description = "Remove a chat's label.")]
    async fn clear_label(&self, Parameters(a): Parameters<ChatArgs>) -> Result<CallToolResult, McpError> {
        Ok(self.run(|x| async move { x.clear_label(&a.chat).await }).await)
    }

    #[tool(
        description = "Set a chat's watch level: muted (never pending, hidden) or normal (also un-hides chats muted in the Instagram app). Never change several without asking."
    )]
    async fn set_watch_level(&self, Parameters(a): Parameters<WatchArgs>) -> Result<CallToolResult, McpError> {
        Ok(self.run(|x| async move { x.set_watch(&a.chat, a.level).await }).await)
    }

    #[tool(
        description = "Call before drafting for a chat. Returns the chat's category, the saved tone profile and chat notes, and profileStale, notesStale (also true with no notes) and needsLearning. When learning is needed it also returns the user's own recent messages in this chat (my_messages_in_chat, up to 60, fetched live) and, when needsLearning, from other chats of the category (my_messages_in_category, up to 40)."
    )]
    async fn get_voice(&self, Parameters(a): Parameters<ChatArgs>) -> Result<CallToolResult, McpError> {
        Ok(self.run(|x| async move { x.get_voice(&a.chat).await }).await)
    }

    #[tool(
        description = "The user's own recent text messages, newest first, fetched live from one chat or from up to 3 chats of a category. Give exactly one of chat or category. Excludes messages insta sent."
    )]
    async fn get_my_messages(&self, Parameters(a): Parameters<MyMessagesArgs>) -> Result<CallToolResult, McpError> {
        let n = limit(a.limit, 60, 200);
        Ok(self.run(|x| async move { x.get_my_messages(a.chat.as_deref(), a.category.as_deref(), n).await }).await)
    }

    #[tool(
        description = "Save the tone profile for a category (replaces the previous one). Describe the style; do not copy private facts verbatim."
    )]
    async fn save_profile(&self, Parameters(a): Parameters<ProfileArgs>) -> Result<CallToolResult, McpError> {
        Ok(self.run(|x| async move { x.save_profile(&a.category, &a.body) }).await)
    }

    #[tool(
        description = "Save notes about how the user writes in one chat (replaces the previous ones). Describe the style; do not copy private facts verbatim."
    )]
    async fn save_chat_notes(&self, Parameters(a): Parameters<NotesArgs>) -> Result<CallToolResult, McpError> {
        Ok(self.run(|x| async move { x.save_chat_notes(&a.chat, &a.body).await }).await)
    }

    #[tool(description = "Set a chat's category (a slug like friends, family, work). Only after the user agrees.")]
    async fn set_category(&self, Parameters(a): Parameters<CategoryArgs>) -> Result<CallToolResult, McpError> {
        Ok(self.run(|x| async move { x.set_category(&a.chat, &a.category).await }).await)
    }

    #[tool(description = "Categories whose saved tone profile is due for a refresh.")]
    async fn list_stale_profiles(&self) -> Result<CallToolResult, McpError> {
        Ok(self.run(|x| async move { x.list_stale_profiles().map(|categories| Categories { categories }) }).await)
    }

    #[tool(
        description = "Transcribe one voice note with OpenAI Whisper and return the text. Costs money: only when the user wants it. Nothing is saved."
    )]
    async fn transcribe_voice(&self, Parameters(a): Parameters<TranscribeArgs>) -> Result<CallToolResult, McpError> {
        Ok(self.run(|x| async move { x.transcribe_voice(&a.chat, &a.item_id).await }).await)
    }
}

#[tool_handler]
impl ServerHandler for InstaServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("insta", env!("CARGO_PKG_VERSION")))
            .with_instructions(current_instructions(self.style_path.as_deref()))
    }
}
