use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    version,
    about = "Harmion node client",
    after_help = "Request files require an owner-only parent directory and must not exist."
)]
pub(crate) struct Cli {
    #[arg(long, value_name = "DIR")]
    pub node: String,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Read node identity.
    Identity,
    /// Read supported capabilities.
    Capabilities,
    /// Read current permissions.
    Permissions,
    /// List cluster members.
    Members,
    /// Join using an owner-verified peer registration.
    Join { path: String },
    /// Join for explicit local owner recovery.
    JoinRecovery { path: String },
    /// Export into a new owner-only directory.
    Export { path: String },
    /// Register an OAuth application.
    OauthRegister { name: String, redirect: String },
    /// Approve an OAuth consent request.
    OauthConsent { request: String },
    /// Get content.
    Get { id: String },
    /// Read content conflicts.
    Conflicts { id: String },
    /// Inspect content history.
    Inspect { id: String },
    /// Synchronize with a peer.
    Sync { id: String },
    /// Read historical replication confirmations (owner only; does not contact peers).
    SyncStatus {
        /// Check a saved update ID, not a Content ID. Unconfirmed does not mean absent.
        #[arg(long, value_name = "UPDATE_ID")]
        update: Option<String>,
    },
    /// Retire a peer.
    Retire { id: String },
    /// Dismiss retirement evidence.
    DismissRetirement { id: String },
    /// Confirm retirement evidence.
    ConfirmRetirement { id: String },
    /// Approve a peer registration.
    Approve { path: String },
    /// Change application grants.
    ChangeGrants { path: String },
    /// Trust a peer for explicit local owner recovery.
    TrustRecoveryPeer { path: String },
    /// Replay a persisted save request.
    Save { path: String },
    /// Create content from a body file.
    Create {
        source: String,
        request_file: String,
        kinds: Vec<String>,
    },
    /// Edit content from a body file.
    Edit {
        id: String,
        token: String,
        source: String,
        request_file: String,
    },
    /// Restore content, replacing all known fields with the supplied body.
    Restore {
        id: String,
        token: String,
        source: String,
        request_file: String,
    },
    /// Delete content.
    Delete {
        id: String,
        token: String,
        request_file: String,
    },
    /// Keep content against a concurrent deletion.
    Keep {
        id: String,
        token: String,
        request_file: String,
    },
    /// Resolve body versions using a body file.
    Resolve {
        id: String,
        token: String,
        source: String,
        request_file: String,
        #[arg(required = true, num_args = 1..)]
        versions: Vec<String>,
    },
    /// Select existing values from a JSON file.
    Select {
        id: String,
        token: String,
        selections: String,
        request_file: String,
    },
    /// Read merge inputs.
    MergeBase {
        id: String,
        token: String,
        left: String,
        right: String,
        base: Option<String>,
    },
    /// Plan a merge.
    MergePlan {
        id: String,
        token: String,
        left: String,
        right: String,
        base: Option<String>,
    },
}

impl Command {
    pub fn operation(&self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Capabilities => "capabilities",
            Self::Permissions => "permissions",
            Self::Members => "members",
            Self::Join { .. } => "join",
            Self::JoinRecovery { .. } => "join-recovery",
            Self::Export { .. } => "export",
            Self::OauthRegister { .. } => "oauth-register",
            Self::OauthConsent { .. } => "oauth-consent",
            Self::Get { .. } => "get",
            Self::Conflicts { .. } => "conflicts",
            Self::Inspect { .. } => "inspect",
            Self::Sync { .. } => "sync",
            Self::SyncStatus { .. } => "sync-status",
            Self::Retire { .. } => "retire",
            Self::DismissRetirement { .. } => "dismiss-retirement",
            Self::ConfirmRetirement { .. } => "confirm-retirement",
            Self::Approve { .. } => "approve",
            Self::ChangeGrants { .. } => "change-grants",
            Self::TrustRecoveryPeer { .. } => "trust-recovery-peer",
            Self::Save { .. } => "save",
            Self::Create { .. } => "create",
            Self::Edit { .. } => "edit",
            Self::Restore { .. } => "restore",
            Self::Delete { .. } => "delete",
            Self::Keep { .. } => "keep",
            Self::Resolve { .. } => "resolve",
            Self::Select { .. } => "select",
            Self::MergeBase { .. } => "merge-base",
            Self::MergePlan { .. } => "merge-plan",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(
            ["harmion", "--node", "node"]
                .into_iter()
                .chain(arguments.iter().copied()),
        )
    }

    #[test]
    fn resolve_requires_versions_and_preserves_their_order() {
        assert!(parse(&["resolve", "id", "token", "body", "request"]).is_err());
        let cli = parse(&["resolve", "id", "token", "body", "request", "v2", "v1"]).unwrap();
        let Command::Resolve { versions, .. } = cli.command else {
            panic!("expected resolve")
        };
        assert_eq!(versions, ["v2", "v1"]);
    }

    #[test]
    fn sync_status_accepts_an_optional_update_id() {
        let cli = parse(&["sync-status"]).unwrap();
        assert!(matches!(cli.command, Command::SyncStatus { update: None }));
        let cli = parse(&["sync-status", "--update", "update-id"]).unwrap();
        assert!(
            matches!(cli.command, Command::SyncStatus { update: Some(id) } if id == "update-id")
        );
        assert!(parse(&["sync-status", "content-id"]).is_err());
        assert!(parse(&["sync-status", "--update"]).is_err());
    }

    #[test]
    fn merge_base_accepts_at_most_one_explicit_base() {
        for operation in ["merge-base", "merge-plan"] {
            assert!(parse(&[operation, "id", "token", "left", "right"]).is_ok());
            assert!(parse(&[operation, "id", "token", "left", "right", "base"]).is_ok());
            assert!(parse(&[operation, "id", "token", "left", "right", "base", "extra"]).is_err());
        }
    }

    #[test]
    fn help_and_argument_errors_do_not_require_a_node_directory() {
        assert_eq!(
            Cli::try_parse_from(["harmion", "--help"])
                .err()
                .unwrap()
                .kind(),
            clap::error::ErrorKind::DisplayHelp
        );
        assert_eq!(
            parse(&["edit", "id"]).err().unwrap().kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        assert_eq!(
            parse(&["unknown"]).err().unwrap().kind(),
            clap::error::ErrorKind::InvalidSubcommand
        );
        assert!(Cli::try_parse_from(["harmion", "identity"]).is_err());
    }
}
