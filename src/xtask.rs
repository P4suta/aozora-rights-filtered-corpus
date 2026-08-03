use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, Utc};
use clap::{Parser, Subcommand};

use aozora_rights_filtered_corpus::generator::{self, Input};

#[derive(Debug, Parser)]
#[command(about = "Reproducible rights-filtered corpus maintenance tasks")]
struct Cli {
    #[command(subcommand)]
    command: Task,
}

#[derive(Debug, Subcommand)]
enum Task {
    Check,
    Format,
    FormatCheck,
    Spellcheck,
    Scan {
        #[arg(long, default_value = "corpus.toml")]
        config: PathBuf,
        #[arg(long)]
        upstream_root: Option<PathBuf>,
        #[arg(long)]
        list_candidates: bool,
    },
    Snapshot {
        #[arg(long, default_value = "corpus.toml")]
        config: PathBuf,
        #[arg(long)]
        upstream_root: Option<PathBuf>,
        #[arg(long, default_value_t = 12)]
        jobs: usize,
    },
    Verify {
        #[arg(long, default_value = "corpus.toml")]
        config: PathBuf,
    },
    Diff {
        #[arg(long, default_value = "corpus.toml")]
        config: PathBuf,
        #[arg(long)]
        base_manifest: Option<PathBuf>,
    },
    Refresh {
        #[arg(long, default_value = "corpus.toml")]
        config: PathBuf,
        #[arg(long)]
        reference_date: Option<NaiveDate>,
        #[arg(long, default_value_t = 20)]
        jobs: usize,
    },
    ProposeUpdate {
        #[arg(long, default_value = "corpus.toml")]
        config: PathBuf,
    },
    Doctor,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Task::Check => check(),
        Task::Format => run("cargo", ["fmt", "--all"]),
        Task::FormatCheck => run("cargo", ["fmt", "--all", "--", "--check"]),
        Task::Spellcheck => spellcheck(),
        Task::Scan {
            config,
            upstream_root,
            list_candidates,
        } => {
            let config_value = generator::load_config(&config)?;
            let report = generator::scan(&config_value, input(upstream_root))?;
            println!(
                "rows={} edition-groups={} candidates={} quarantined={}",
                report.csv_rows,
                report.edition_groups,
                report.candidates.len(),
                report.quarantine.len()
            );
            if list_candidates {
                for candidate in report.candidates.iter().filter(|candidate| {
                    candidate.title.is_empty()
                        || candidate.title.trim() != candidate.title
                        || candidate.reading.is_empty()
                        || candidate.reading.trim() != candidate.reading
                }) {
                    println!(
                        "{}\t{}\t{}\t{}",
                        candidate.edition_id,
                        candidate.title,
                        candidate.reading,
                        candidate.first_publication.raw
                    );
                }
            }
            Ok(())
        }
        Task::Snapshot {
            config,
            upstream_root,
            jobs,
        } => generator::snapshot(&config, input(upstream_root), jobs),
        Task::Verify { config } => generator::verify(&config),
        Task::Diff {
            config,
            base_manifest,
        } => {
            println!(
                "{}",
                generator::diff_summary(&config, base_manifest.as_deref())?
            );
            Ok(())
        }
        Task::Refresh {
            config,
            reference_date,
            jobs,
        } => generator::refresh(
            &config,
            reference_date.unwrap_or_else(|| Utc::now().date_naive()),
            jobs,
        ),
        Task::ProposeUpdate { config } => propose_update(&config),
        Task::Doctor => doctor(),
    }
}

fn input(upstream_root: Option<PathBuf>) -> Input {
    upstream_root.map_or(Input::Remote, Input::Checkout)
}

fn check() -> Result<()> {
    run("cargo", ["fmt", "--all", "--", "--check"])?;
    spellcheck()?;
    run("cargo", ["check", "--locked", "--all-targets"])?;
    run(
        "cargo",
        [
            "clippy",
            "--locked",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run("cargo", ["test", "--locked", "--all-targets"])?;
    if Path::new("corpus/manifest.json").exists() {
        generator::verify(Path::new("corpus.toml"))?;
    }
    Ok(())
}

fn spellcheck() -> Result<()> {
    run("typos", std::iter::empty::<&str>())
}

fn doctor() -> Result<()> {
    for program in ["cargo", "git"] {
        let status = Command::new(program)
            .arg("--version")
            .status()
            .with_context(|| format!("run {program} --version"))?;
        success(program, status)?;
    }
    if env::var_os("CI").is_some()
        && env::var_os("GITHUB_TOKEN").is_none()
        && env::var_os("GH_TOKEN").is_none()
    {
        bail!("GITHUB_TOKEN or GH_TOKEN is required by CI update jobs");
    }
    generator::load_config(Path::new("corpus.toml"))?;
    println!("doctor: host tools and corpus policy are valid");
    Ok(())
}

fn propose_update(config_path: &Path) -> Result<()> {
    if env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        bail!("propose-update is restricted to an ephemeral GitHub Actions checkout");
    }
    let config = generator::load_config(config_path)?;
    let status = output("git", ["status", "--porcelain=v1"])?;
    if status.trim().is_empty() {
        println!("propose-update: snapshot is unchanged");
        return Ok(());
    }
    for line in status.lines() {
        let path = line
            .get(3..)
            .context("unexpected git status porcelain record")?;
        if path != "corpus.toml" && !path.starts_with("corpus/") {
            bail!("refusing to commit an unexpected path: {path}");
        }
    }
    let run_id = env::var("GITHUB_RUN_ID").context("GITHUB_RUN_ID is required")?;
    if !run_id.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("GITHUB_RUN_ID is not numeric");
    }
    let branch = format!(
        "automation/corpus-update-{}-{run_id}",
        config.policy.reference_date
    );
    run("git", ["switch", "-c", branch.as_str()])?;
    run("git", ["config", "user.name", "github-actions[bot]"])?;
    run(
        "git",
        [
            "config",
            "user.email",
            "41898282+github-actions[bot]@users.noreply.github.com",
        ],
    )?;
    run("git", ["add", "--", "corpus.toml", "corpus"])?;
    run(
        "git",
        [
            "commit",
            "-m",
            &format!(
                "chore: update corpus snapshot {}",
                config.policy.reference_date
            ),
        ],
    )?;
    run("git", ["push", "--set-upstream", "origin", branch.as_str()])?;
    run(
        "gh",
        [
            "pr",
            "create",
            "--base",
            "main",
            "--head",
            branch.as_str(),
            "--title",
            &format!(
                "chore: update corpus snapshot {}",
                config.policy.reference_date
            ),
            "--body-file",
            "corpus/update-report.md",
        ],
    )
}

fn output<I, S>(program: &str, arguments: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(program)
        .args(arguments)
        .output()
        .with_context(|| format!("run {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).with_context(|| format!("{program} output is not UTF-8"))
}

fn run<I, S>(program: &str, arguments: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let status = Command::new(program)
        .args(arguments)
        .status()
        .with_context(|| format!("run {program}"))?;
    success(program, status)
}

fn success(program: &str, status: ExitStatus) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        bail!("{program} exited with {status}")
    }
}
