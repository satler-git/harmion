//! Out-of-band owner approval on the terminal that launched the admin server.
//!
//! The browser session alone must not complete privilege, trust or export
//! changes. The server asks the launcher's stdin for a single-use confirmation
//! bound to the operation it is about to execute.

use clap::ValueEnum;
use std::{
    io::{IsTerminal, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

const APPROVAL_TTL: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ApprovalMode {
    /// Require an interactive terminal. Non-TTY launchers deny R2.
    Terminal,
    /// Read approvals from stdin under a supervisor (tests, automation).
    Stdio,
    /// Skip the out-of-band prompt and rely on the browser session. Less secure:
    /// a stolen admin session can complete R2 without an additional check.
    Session,
    /// Deny every R2 operation.
    Deny,
}

impl ApprovalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Stdio => "stdio",
            Self::Session => "session",
            Self::Deny => "deny",
        }
    }
}

pub struct Status {
    pub mode: &'static str,
    /// True when this mode expects an out-of-band prompt at all.
    pub terminal_required: bool,
    /// True when the prompt channel is actually available.
    pub terminal_available: bool,
}

struct Request {
    operation: String,
    summary: String,
    reply: oneshot::Sender<bool>,
}

#[derive(Clone)]
pub struct Confirmer {
    sender: Option<mpsc::Sender<Request>>,
    poisoned: Arc<AtomicBool>,
    mode: ApprovalMode,
}

impl Confirmer {
    pub fn start(mode: ApprovalMode) -> Self {
        let poisoned = Arc::new(AtomicBool::new(false));
        if mode == ApprovalMode::Deny
            || mode == ApprovalMode::Session
            || (mode == ApprovalMode::Terminal && !std::io::stdin().is_terminal())
        {
            return Self {
                sender: None,
                poisoned,
                mode,
            };
        }
        let (sender, mut receiver) = mpsc::channel::<Request>(1);
        std::thread::spawn(move || {
            while let Some(request) = receiver.blocking_recv() {
                tracing::info!(operation = %request.operation, "owner approval prompt shown");
                eprint!(
                    "\n[harmion-admin] 所有者の承認が必要です\n{}\n承認しますか? [y/N] ",
                    request.summary
                );
                let _ = std::io::stderr().flush();
                let mut input = String::new();
                let read = std::io::stdin().read_line(&mut input);
                let approved = read.is_ok() && matches!(input.trim(), "y" | "Y" | "yes" | "YES");
                tracing::info!(
                    operation = %request.operation,
                    approved,
                    reason = if approved { "approved" } else { "owner_declined" },
                    "owner approval decision"
                );
                let _ = request.reply.send(approved);
            }
        });
        Self {
            sender: Some(sender),
            poisoned,
            mode,
        }
    }

    /// Returns false on deny, timeout, a closed launcher, or a poisoned prompt.
    /// In `session` mode the browser session is the only check, so this returns
    /// true without waiting for stdin.
    pub async fn confirm(&self, operation: &str, summary: String) -> bool {
        if self.mode == ApprovalMode::Session {
            tracing::warn!(
                operation,
                reason = "session_only_approval",
                "owner approval completed without an out-of-band prompt"
            );
            return true;
        }
        if self.poisoned.load(Ordering::Relaxed) {
            tracing::warn!(
                operation,
                reason = "approval_channel_poisoned",
                "owner approval denied"
            );
            return false;
        }
        let Some(sender) = &self.sender else {
            tracing::warn!(
                operation,
                reason = "approval_channel_unavailable",
                "owner approval denied"
            );
            return false;
        };
        let (reply, response) = oneshot::channel();
        if sender
            .send(Request {
                operation: operation.to_owned(),
                summary,
                reply,
            })
            .await
            .is_err()
        {
            tracing::warn!(
                operation,
                reason = "approval_channel_closed",
                "owner approval denied"
            );
            return false;
        }
        match tokio::time::timeout(APPROVAL_TTL, response).await {
            Ok(Ok(approved)) => approved,
            _ => {
                // A timed-out stdin read may still consume the next line. Deny
                // all later approvals instead of guessing which request it belongs to.
                tracing::warn!(
                    operation,
                    reason = "approval_timeout",
                    timeout_secs = APPROVAL_TTL.as_secs(),
                    "owner approval timed out; denying later approvals"
                );
                self.poisoned.store(true, Ordering::Relaxed);
                false
            }
        }
    }

    pub fn status(&self) -> Status {
        Status {
            mode: self.mode.as_str(),
            terminal_required: matches!(self.mode, ApprovalMode::Terminal | ApprovalMode::Stdio),
            terminal_available: self.sender.is_some(),
        }
    }

    pub fn unavailable(&self) -> Option<&'static str> {
        if self.mode == ApprovalMode::Session || self.sender.is_some() {
            return None;
        }
        Some(match self.mode {
            ApprovalMode::Terminal => {
                "R2には起動端末のTTYが必要です。--approval=stdioは監督下でのみ使ってください"
            }
            ApprovalMode::Stdio => "承認チャネルが開始できませんでした",
            ApprovalMode::Session => "端末承認なしで実行します（--approval=session）",
            ApprovalMode::Deny => "R2は--approval=denyで拒否されています",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deny_mode_never_approves() {
        let confirmer = Confirmer::start(ApprovalMode::Deny);
        assert!(!confirmer.confirm("/test", "test summary".into()).await);
        assert!(confirmer.unavailable().is_some());
        assert!(!confirmer.status().terminal_required);
    }

    #[tokio::test]
    async fn session_mode_approves_without_a_prompt() {
        let confirmer = Confirmer::start(ApprovalMode::Session);
        assert!(confirmer.confirm("/test", "test summary".into()).await);
        assert!(confirmer.unavailable().is_none());
        assert_eq!(confirmer.status().mode, "session");
        assert!(!confirmer.status().terminal_required);
    }
}
