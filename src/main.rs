use std::process::ExitCode;

use clap::Parser;

use indeks::cli::Cli;
use indeks::error::Result;
use indeks::{http, report, sitemap, targets, validate};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            err.exit_code()
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let args = cli.engine.args();

    // Before validation, so that --verbose covers the whole run.
    http::init_tracing(args.verbose);

    let validated = validate::validate(args, cli.engine.kind())?;

    if validated.dry_run {
        let previews = sitemap::preview(&validated.sitemaps)?;
        report::dry_run(cli.engine.name(), &validated, &previews);
        return Ok(());
    }

    let client = http::client()?;
    let submitter = cli
        .engine
        .kind()
        .build(&validated.credential, client.clone())?;

    let targets = targets::collect(&validated, &client)?;
    tracing::info!("submitting {} URLs to {}", targets.len(), submitter.name());

    let outcomes = submitter.submit(&targets)?;
    report::report(&outcomes, validated.verbose)
}
