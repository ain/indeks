//! Argument definitions.
//!
//! Values arrive here as raw strings rather than parsed `Url`s or `PathBuf`s on
//! purpose: clap would abort on the first malformed value, whereas the spec asks
//! for every input problem to be reported in one go. Parsing happens in
//! [`crate::validate`].

use clap::{Args, Parser, Subcommand};

/// Push URLs and sitemaps to search engines for indexing.
#[derive(Debug, Parser)]
#[command(name = "indeks", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub engine: EngineCommand,
}

#[derive(Debug, Subcommand)]
pub enum EngineCommand {
    /// Submit to Google through the Indexing API.
    Google(SubmissionArgs),

    /// Submit to Bing through IndexNow.
    Bing(SubmissionArgs),
}

impl EngineCommand {
    /// The arguments given to whichever engine was selected.
    pub fn args(&self) -> &SubmissionArgs {
        match self {
            EngineCommand::Google(args) | EngineCommand::Bing(args) => args,
        }
    }

    /// Which engine was selected.
    pub fn kind(&self) -> crate::engine::Kind {
        match self {
            EngineCommand::Google(_) => crate::engine::Kind::Google,
            EngineCommand::Bing(_) => crate::engine::Kind::Bing,
        }
    }

    /// Name of the selected engine, as shown in output.
    pub fn name(&self) -> &'static str {
        self.kind().name()
    }
}

#[derive(Debug, Args)]
pub struct SubmissionArgs {
    /// Absolute URL to submit. May be repeated.
    #[arg(long = "url", value_name = "URL")]
    pub urls: Vec<String>,

    /// Sitemap whose <loc> entries are submitted. May be repeated.
    #[arg(long = "sitemap", value_name = "SITEMAP_URL")]
    pub sitemaps: Vec<String>,

    /// API token, or path to a JSON credentials file.
    #[arg(long, value_name = "TOKEN_OR_PATH")]
    pub credentials: Option<String>,

    /// Check the input without contacting any external system.
    #[arg(long)]
    pub dry_run: bool,

    /// Log all network activity, including headers and bodies.
    #[arg(long)]
    pub verbose: bool,
}
