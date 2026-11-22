use clap::{Args, Parser, Subcommand, ValueEnum};
use std::net::SocketAddr;

#[derive(Parser)]
#[command(version, about = "Harmion node daemon")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Initialize a node directory.
    Init {
        dir: String,
        #[arg(long)]
        cluster: Option<String>,
    },
    /// Restore a backup into a new node directory.
    Restore { backup_dir: String, new_dir: String },
    /// Provision the OAuth browser TLS identity and print its trust root.
    OauthProvision {
        dir: String,
        #[arg(long)]
        replace: bool,
    },
    /// Print the OAuth browser trust root as PEM.
    OauthRoot { dir: String },
    /// Enroll the first owner SSH key. Run while the daemon is stopped.
    OwnerEnroll {
        dir: String,
        #[arg(long, value_name = "PATH")]
        ssh_key: String,
        #[arg(long, default_value = "owner")]
        label: String,
    },
    /// Run a node, overriding saved settings when options are supplied.
    Run {
        dir: String,
        #[command(flatten)]
        options: RunOptions,
    },
}

#[derive(Args, Debug, Default)]
pub(crate) struct RunOptions {
    #[arg(long)]
    pub app_listen: Option<SocketAddr>,
    #[arg(long, value_name = "127.0.0.1:PORT|off")]
    pub oauth_listen: Option<OAuthListen>,
    #[arg(long)]
    pub peer_listen: Option<SocketAddr>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(0..=3600))]
    pub sync_seconds: Option<u64>,
    #[arg(long, action = clap::ArgAction::Set)]
    pub bootstrap_cluster_root: Option<bool>,
    #[arg(long, value_enum)]
    pub peer_transport: Option<PeerTransport>,
    #[arg(long)]
    pub relay_url: Option<String>,
    #[arg(long, value_enum)]
    pub discovery: Option<Discovery>,
    /// Additional DER trust anchors for signed-HTTPS peers (private CA).
    #[arg(long = "signed-https-root", value_name = "PATH")]
    pub signed_https_roots: Vec<std::path::PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum PeerTransport {
    Iroh,
    Https,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Discovery {
    Public,
    None,
}

/// An omitted option retains the saved address; `off` explicitly disables it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OAuthListen(Option<SocketAddr>);

impl OAuthListen {
    pub fn address(self) -> Option<SocketAddr> {
        self.0
    }
}

impl std::str::FromStr for OAuthListen {
    type Err = std::net::AddrParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "off" {
            Ok(Self(None))
        } else {
            value.parse().map(|address| Self(Some(address)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_run_overrides_and_explicit_off() {
        let cli = Cli::try_parse_from([
            "harmiond",
            "run",
            "node",
            "--oauth-listen",
            "off",
            "--bootstrap-cluster-root",
            "false",
            "--sync-seconds",
            "0",
            "--peer-transport",
            "https",
            "--discovery",
            "none",
        ])
        .unwrap();
        let Command::Run { dir, options } = cli.command else {
            panic!("expected run")
        };
        assert_eq!(dir, "node");
        assert!(options.oauth_listen.unwrap().address().is_none());
        assert_eq!(options.bootstrap_cluster_root, Some(false));
        assert_eq!(options.sync_seconds, Some(0));
        assert!(matches!(options.peer_transport, Some(PeerTransport::Https)));
        assert!(matches!(options.discovery, Some(Discovery::None)));
    }

    #[test]
    fn omitted_overrides_preserve_saved_settings() {
        let cli = Cli::try_parse_from(["harmiond", "run", "node"]).unwrap();
        let Command::Run { options, .. } = cli.command else {
            panic!("expected run")
        };
        assert!(options.oauth_listen.is_none());
        assert!(options.bootstrap_cluster_root.is_none());
        assert!(options.sync_seconds.is_none());
    }

    #[test]
    fn rejects_invalid_options_before_opening_node() {
        for args in [
            vec!["--sync-seconds", "3601"],
            vec!["--peer-transport", "other"],
            vec!["--discovery", "other"],
            vec!["--oauth-listen", "invalid"],
            vec!["--bootstrap-cluster-root"],
            vec!["--unknown", "value"],
        ] {
            assert!(
                Cli::try_parse_from(["harmiond", "run", "node"].into_iter().chain(args)).is_err()
            );
        }
    }

    #[test]
    fn signed_https_roots_are_repeatable() {
        let cli = Cli::try_parse_from([
            "harmiond",
            "run",
            "node-dir",
            "--signed-https-root",
            "ca.der",
            "--signed-https-root",
            "other.der",
        ])
        .unwrap();
        match cli.command {
            Command::Run { options, .. } => assert_eq!(options.signed_https_roots.len(), 2),
            _ => panic!("run command"),
        }
    }
}
