use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    version,
    about = "Harmion node client",
    after_help = "Request files require an owner-only parent directory and must not exist."
)]
pub(crate) struct Cli {
    /// Node directory, or a name under ~/.local/share/harmion. Defaults to main.
    #[arg(long, value_name = "DIR|NAME")]
    pub node: Option<String>,
    /// OpenSSH public key whose private half is held by ssh-agent, or a private
    /// key file. Defaults to HARMION_OWNER_SSH_KEY, an ssh-agent identity, or ~/.ssh/id_ed25519.pub.
    #[arg(long, value_name = "PATH")]
    pub owner_key: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Manage local nodes.
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
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
    /// Export the provisioning manifest for an approved signed-HTTPS peer.
    ProvisionManifest {
        /// Peer registration JSON file, or a harmion-peer:v1:z: code.
        peer: String,
        /// Write the manifest to this file instead of stdout.
        #[arg(long, value_name = "PATH")]
        out: Option<String>,
    },
    /// Change application grants.
    ChangeGrants { path: String },
    /// List Interpreter providers visible to this app.
    InterpreterList,
    /// Describe an Interpreter provider by ID.
    InterpreterDescribe { id: String },
    /// Register or update this app's interpreter manifest from a JSON file.
    InterpreterRegister { path: String },
    /// Approve a pending Interpreter provider (owner only).
    InterpreterApprove {
        id: String,
        #[arg(long)]
        replace: bool,
    },
    /// Call an Interpreter using a JSON request file.
    InterpreterCall { request: String },
    /// Cancel one of this app's interpreter invocations.
    InterpreterCancel { id: String },
    /// List integrated connection requests.
    ConnectList,
    /// Approve or deny an integrated connection request (owner only).
    ConnectApprove {
        id: String,
        #[arg(long)]
        deny: bool,
        #[arg(long)]
        replace: bool,
    },
    /// Trust a peer for explicit local owner recovery.
    TrustRecoveryPeer { path: String },
    /// Show owner credentials, enforcement and recent approvals.
    OwnerStatus,
    /// Add a backup owner SSH public key (requires an existing owner approval).
    OwnerEnroll {
        key: String,
        #[arg(long, default_value = "owner")]
        label: String,
    },
    /// Revoke an owner credential by its ID (requires an existing approval).
    OwnerRevoke { credential: String },
    /// Recover ownership with the offline recovery code and a new SSH key.
    OwnerRecover {
        code: String,
        key: String,
        #[arg(long, default_value = "owner")]
        label: String,
    },
    /// Enable or disable owner enforcement.
    OwnerEnforce {
        #[arg(action = clap::ArgAction::Set)]
        enable: bool,
    },
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

#[derive(Subcommand)]
pub(crate) enum NodeCommand {
    /// List initialized local node names.
    List,
}

impl Command {
    pub fn operation(&self) -> &'static str {
        match self {
            Self::Node { .. } => "node",
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
            Self::ProvisionManifest { .. } => "provision-manifest",
            Self::ChangeGrants { .. } => "change-grants",
            Self::InterpreterList => "interpreter-list",
            Self::InterpreterDescribe { .. } => "interpreter-describe",
            Self::InterpreterRegister { .. } => "interpreter-register",
            Self::InterpreterApprove { .. } => "interpreter-approve",
            Self::InterpreterCall { .. } => "interpreter-call",
            Self::InterpreterCancel { .. } => "interpreter-cancel",
            Self::ConnectList => "connect-list",
            Self::ConnectApprove { .. } => "connect-approve",
            Self::TrustRecoveryPeer { .. } => "trust-recovery-peer",
            Self::OwnerStatus => "owner-status",
            Self::OwnerEnroll { .. } => "owner-enroll",
            Self::OwnerRevoke { .. } => "owner-revoke",
            Self::OwnerRecover { .. } => "owner-recover",
            Self::OwnerEnforce { .. } => "owner-enforce",
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
    fn owner_enforce_takes_an_explicit_boolean() {
        let cli = parse(&["owner-enforce", "true"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::OwnerEnforce { enable: true }
        ));
        let cli = parse(&["owner-enforce", "false"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::OwnerEnforce { enable: false }
        ));
        assert!(parse(&["owner-enforce"]).is_err());
        assert!(parse(&["owner-enforce", "maybe"]).is_err());
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
    fn provision_manifest_accepts_a_file_or_code_and_optional_output() {
        let cli = parse(&["provision-manifest", "peer.json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::ProvisionManifest { out: None, .. }
        ));
        let cli = parse(&[
            "provision-manifest",
            "harmion-peer:v1:z:abc",
            "--out",
            "manifest.json",
        ])
        .unwrap();
        let Command::ProvisionManifest { peer, out } = cli.command else {
            panic!("expected provision-manifest")
        };
        assert_eq!(peer, "harmion-peer:v1:z:abc");
        assert_eq!(out.as_deref(), Some("manifest.json"));
        assert!(parse(&["provision-manifest", "peer", "extra"]).is_err());
    }

    #[test]
    fn interpreter_commands_parse_provider_and_invocation_ids() {
        assert!(matches!(
            parse(&["interpreter-list"]).unwrap().command,
            Command::InterpreterList
        ));
        let cli = parse(&["interpreter-describe", "id"]).unwrap();
        assert!(matches!(cli.command, Command::InterpreterDescribe { id } if id == "id"));
        let cli = parse(&["interpreter-approve", "id", "--replace"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::InterpreterApprove { id, replace: true } if id == "id"
        ));
        let cli = parse(&["interpreter-register", "manifest.json"]).unwrap();
        assert!(
            matches!(cli.command, Command::InterpreterRegister { path } if path == "manifest.json")
        );
        let cli = parse(&["interpreter-call", "call.json"]).unwrap();
        assert!(
            matches!(cli.command, Command::InterpreterCall { request } if request == "call.json")
        );
        let cli = parse(&["interpreter-cancel", "id"]).unwrap();
        assert!(matches!(cli.command, Command::InterpreterCancel { id } if id == "id"));
        assert!(parse(&["interpreter-describe"]).is_err());
        assert!(parse(&["interpreter-cancel", "id", "extra"]).is_err());
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
        let cli = Cli::try_parse_from(["harmion", "identity"]).unwrap();
        assert!(cli.node.is_none());
        assert!(matches!(
            Cli::try_parse_from(["harmion", "node", "list"])
                .unwrap()
                .command,
            Command::Node { .. }
        ));
    }
}
