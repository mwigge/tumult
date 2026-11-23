//! `manual` subcommand arguments.

use std::path::PathBuf;

/// How a read prints its answer.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ManualFormat {
    /// Readable text (default)
    #[default]
    Text,
    /// The JSON the daemon answered
    Json,
}

impl ManualFormat {
    pub(crate) fn json(self) -> bool {
        self == Self::Json
    }
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum ManualAction {
    /// List the records, newest entry first
    List {
        /// Only records in this status: draft, submitted, verified or rejected
        #[arg(long)]
        status: Option<String>,
        /// Only records of this exercise type, such as failover or restore
        #[arg(long = "type", value_name = "TYPE")]
        exercise_type: Option<String>,
        /// Only records of this target system
        #[arg(long)]
        target: Option<String>,
        /// Only records executed on or after this date (YYYY-MM-DD, UTC) or
        /// RFC 3339 time
        #[arg(long)]
        from: Option<String>,
        /// Only records executed before this time; a date counts its whole day
        #[arg(long)]
        to: Option<String>,
        /// How to print the answer
        #[arg(long, value_enum, default_value_t)]
        format: ManualFormat,
    },
    /// Show one record with its steps, actions, links, attachments and
    /// sign-offs, and how many sign-offs are still needed
    Show {
        id: String,
        /// How to print the answer
        #[arg(long, value_enum, default_value_t)]
        format: ManualFormat,
    },
    /// Create a draft from a TOON or JSON file with the record body of the
    /// API; a field the API does not know is refused
    Create {
        /// The record body: JSON when the name ends in .json, TOON otherwise
        #[arg(long)]
        file: PathBuf,
        /// Who enters it, while the daemon runs without auth
        #[arg(long)]
        by: Option<String>,
    },
    /// Submit a draft for sign-off
    Submit {
        id: String,
        /// The statement the submitter stands by (default: the one of the draft)
        #[arg(long)]
        attestation: Option<String>,
        /// Who submits it, while the daemon runs without auth
        #[arg(long)]
        by: Option<String>,
    },
    /// Approve or reject a submitted record
    SignOff {
        id: String,
        /// Approve the record
        #[arg(long, required_unless_present = "reject", conflicts_with = "reject")]
        approve: bool,
        /// Reject the record (needs --note)
        #[arg(long, requires = "note")]
        reject: bool,
        /// Why, kept with the sign-off
        #[arg(long)]
        note: Option<String>,
        /// Who signs, while the daemon runs without auth
        #[arg(long)]
        by: Option<String>,
    },
    /// Attach a file, or a link with --link, as evidence
    Attach {
        id: String,
        /// The file: PDF, PNG, JPEG, text, CSV or JSON, at most 25 MB
        #[arg(required_unless_present = "link", conflicts_with = "link")]
        file: Option<PathBuf>,
        /// A link instead of a file
        #[arg(long)]
        link: Option<String>,
        /// The label of the link
        #[arg(long, requires = "link")]
        label: Option<String>,
        /// Who attaches it, while the daemon runs without auth
        #[arg(long)]
        by: Option<String>,
    },
    /// Check the seal, the audit trail, the sign-offs and the files of a
    /// record; exit 1 when it is not ok
    Verify {
        id: String,
        /// How to print the answer
        #[arg(long, value_enum, default_value_t)]
        format: ManualFormat,
    },
    /// Planned exercises: list, show, create and cancel
    Plans {
        #[command(subcommand)]
        action: ManualPlansAction,
    },
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum ManualPlansAction {
    /// List the plans, the soonest due first
    List {
        /// Only plans in this status: planned, fulfilled or cancelled
        #[arg(long)]
        status: Option<String>,
        /// Only plans of this target system
        #[arg(long)]
        target: Option<String>,
        /// How to print the answer
        #[arg(long, value_enum, default_value_t)]
        format: ManualFormat,
    },
    /// Show one plan with its audit trail
    Show {
        id: String,
        /// How to print the answer
        #[arg(long, value_enum, default_value_t)]
        format: ManualFormat,
    },
    /// Create a plan from a TOON or JSON file with the plan body of the API
    Create {
        /// The plan body: JSON when the name ends in .json, TOON otherwise
        #[arg(long)]
        file: PathBuf,
        /// Who creates it, while the daemon runs without auth
        #[arg(long)]
        by: Option<String>,
    },
    /// Cancel a planned plan
    Cancel {
        id: String,
        /// Why, kept in the audit trail of the plan
        #[arg(long)]
        reason: Option<String>,
        /// Who cancels it, while the daemon runs without auth
        #[arg(long)]
        by: Option<String>,
    },
}
