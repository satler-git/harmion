use clap::{Args, Parser, Subcommand};
use mote_core::ArchiveFilter;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "mote",
    version,
    about = "Mote Capture CLI",
    after_help = "Run `mote connect` first. --token-file/MOTE_TOKEN use an existing token; --owner is an explicit local-admin fallback."
)]
pub struct Cli {
    /// Node directory, or a name under ~/.local/share/harmion. Defaults to main.
    #[arg(long, global = true, value_name = "DIR|NAME")]
    pub node: Option<String>,
    /// Capture-only App API access token file (0600, hex).
    #[arg(long, global = true, value_name = "FILE", conflicts_with = "owner")]
    pub token_file: Option<PathBuf>,
    /// Use the node directory's admin credential. This is not capture-only.
    #[arg(long, global = true)]
    pub owner: bool,
    /// Print machine-readable JSON.
    #[arg(long, global = true)]
    pub json: bool,
    /// Disable ANSI colors in human-readable output.
    #[arg(long, global = true)]
    pub no_color: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Capture text. With no TEXT, reads non-interactive stdin.
    Q(QArgs),
    /// List captures.
    #[command(alias = "ls")]
    List(ListArgs),
    /// Search capture bodies.
    Search(SearchArgs),
    /// Print one capture body.
    Show(ShowArgs),
    /// Archive captures.
    Archive { ids: Vec<String> },
    /// Restore archived captures to the inbox.
    Unarchive { ids: Vec<String> },
    /// Pin captures.
    Pin { ids: Vec<String> },
    /// Unpin captures.
    Unpin { ids: Vec<String> },
    /// List captures waiting in the local outbox.
    Pending,
    /// Retry one pending capture by ID prefix, or all pending captures.
    Retry { id: Option<String> },
    /// Discard one pending capture by ID prefix without sending it.
    Forget { id: String },
    /// Connect this node to Mote and store capture-only credentials.
    Connect(ConnectArgs),
    /// Inspect or remove stored Mote credentials.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
}

#[derive(Args)]
pub struct ConnectArgs {
    /// Name shown in the owner's approval screen.
    #[arg(long, default_value = "Mote")]
    pub name: String,
    /// Seconds to wait for owner approval.
    #[arg(long, default_value_t = 600)]
    pub timeout: u64,
    /// Seconds between connection status polls.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..))]
    pub poll: u64,
}

#[derive(Subcommand)]
pub enum AuthCommand {
    /// Show whether credentials exist and when they expire.
    Status,
    /// Remove stored Mote credentials.
    Logout,
}

#[derive(Args)]
pub struct QArgs {
    /// Text to capture; use - to force stdin. With no TEXT, non-interactive stdin is read.
    pub text: Option<String>,
    /// Content ID this capture replies to.
    #[arg(long, value_name = "ID")]
    pub reply_to: Option<String>,
}

#[derive(Args)]
pub struct ListArgs {
    /// Include archived captures.
    #[arg(long, conflicts_with = "archived")]
    pub all: bool,
    /// Show only archived captures.
    #[arg(long, conflicts_with = "all")]
    pub archived: bool,
    /// Restrict to pinned captures (within the selected archive scope).
    #[arg(long)]
    pub pinned: bool,
    /// Maximum rows to print; 0 means all.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// Reverse the chosen order.
    #[arg(long)]
    pub reverse: bool,
    /// Filter bodies by this substring.
    #[arg(long, value_name = "QUERY")]
    pub search: Option<String>,
    /// Print tab-separated rows.
    #[arg(long)]
    pub tsv: bool,
}

#[derive(Args)]
pub struct SearchArgs {
    /// Substring to find in capture bodies.
    pub query: String,
    /// Include archived captures.
    #[arg(long, conflicts_with = "archived")]
    pub all: bool,
    /// Search only archived captures.
    #[arg(long, conflicts_with = "all")]
    pub archived: bool,
    /// Restrict to pinned captures (within the selected archive scope).
    #[arg(long)]
    pub pinned: bool,
    /// Maximum rows to print; 0 means all.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// Reverse the chosen order.
    #[arg(long)]
    pub reverse: bool,
    /// Print tab-separated rows.
    #[arg(long)]
    pub tsv: bool,
}

#[derive(Args)]
pub struct ShowArgs {
    /// Full content ID or an unambiguous prefix.
    pub id: String,
    /// Prefix the body with metadata.
    #[arg(long)]
    pub meta: bool,
    /// Print tab-separated rows.
    #[arg(long)]
    pub tsv: bool,
}

impl ListArgs {
    pub fn archive_filter(&self) -> ArchiveFilter {
        if self.archived {
            ArchiveFilter::Archived
        } else if self.all {
            ArchiveFilter::All
        } else {
            ArchiveFilter::Inbox
        }
    }

    pub fn order(&self) -> mote_core::Sort {
        match self.archive_filter() {
            ArchiveFilter::Archived => mote_core::Sort::Archive,
            ArchiveFilter::All => mote_core::Sort::Created,
            ArchiveFilter::Inbox => mote_core::Sort::Inbox,
        }
    }
}

impl SearchArgs {
    pub fn archive_filter(&self) -> ArchiveFilter {
        if self.archived {
            ArchiveFilter::Archived
        } else if self.all {
            ArchiveFilter::All
        } else {
            ArchiveFilter::Inbox
        }
    }

    pub fn order(&self) -> mote_core::Sort {
        match self.archive_filter() {
            ArchiveFilter::Archived => mote_core::Sort::Archive,
            ArchiveFilter::All => mote_core::Sort::Created,
            ArchiveFilter::Inbox => mote_core::Sort::Inbox,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn list_has_an_ls_alias() {
        let cli = Cli::parse_from(["mote", "ls"]);
        assert!(matches!(cli.command, Command::List(_)));
    }

    #[test]
    fn list_archive_flags_are_exclusive() {
        assert!(Cli::try_parse_from(["mote", "list", "--all", "--archived"]).is_err());
        assert!(Cli::try_parse_from(["mote", "--json", "--tsv", "list"]).is_err());
        assert!(Cli::try_parse_from(["mote", "search", "x", "--all", "--archived"]).is_err());
    }

    #[test]
    fn search_defaults_to_the_inbox() {
        let cli = Cli::parse_from(["mote", "search", "x"]);
        let Command::Search(args) = cli.command else {
            panic!("expected search");
        };
        assert_eq!(args.archive_filter(), ArchiveFilter::Inbox);
    }

    #[test]
    fn defaults_to_inbox_and_twenty_rows() {
        let cli = Cli::parse_from(["mote", "list"]);
        let Command::List(args) = cli.command else {
            panic!("expected list");
        };
        assert_eq!(args.archive_filter(), ArchiveFilter::Inbox);
        assert_eq!(args.limit, 20);
    }
}
