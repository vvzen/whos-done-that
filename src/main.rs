use std::fmt::Display;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Parser;
use color_eyre::{eyre, Section};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(
    version,
    long_about = "A CLI to help establish ownernship of codebases",
    arg_required_else_help = true
)]
struct Cli {
    #[arg(
        short,
        long,
        help = "The target directory to analyze. It must be a git repo."
    )]
    target_dir: Option<PathBuf>,
}

struct CodeEdit {
    additions: usize,
    removals: usize,
}

impl Display for CodeEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = format!(
            "{} additions and {} removals",
            self.additions, self.removals
        );
        write!(f, "{s}")
    }
}

fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;

    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .finish();

    tracing::subscriber::set_global_default(subscriber)?;

    let target_dir = cli.target_dir.unwrap_or(cwd);
    let authors = get_all_authors(&target_dir)?;

    let mut authors_num_commits = Vec::new();

    // TODO: This could be parallelized with rayon
    for author in authors {
        let num_commits = get_num_author_commits(&author, target_dir.as_path())?;
        let code_edits = get_num_author_edits(&author, target_dir.as_path())?;
        authors_num_commits.push((author.to_owned(), num_commits, code_edits));
    }
    authors_num_commits.sort_by(|a, b| a.1.cmp(&b.1).reverse());

    // Print the final stats
    let stdout = std::io::stdout();
    let mut stdout_handle = std::io::BufWriter::new(stdout);

    for (author, num_commits, code_edits) in authors_num_commits {
        let ending = match num_commits {
            0 => "no commits".to_string(),
            1 => "1 commit".to_string(),
            _ => format!("{num_commits} commits"),
        };

        writeln!(stdout_handle, "{author} has made {ending}: {code_edits}")?;
    }

    Ok(())
}

/// Return a list sorted lexicographically containing all of the detected authors
/// for the git repository living at `target_dir`.
fn get_all_authors(target_dir: impl AsRef<Path>) -> eyre::Result<Vec<String>> {
    let old_cwd = std::env::current_dir()?;
    std::env::set_current_dir(target_dir.as_ref())?;

    let command = "git shortlog --summary --numbered --all --no-merges";
    let stdout = get_stdout_from_subprocess_or_fail(command)?;

    let mut authors = stdout
        .lines()
        .filter_map(|l| {
            let author_tokens = l
                .split_ascii_whitespace()
                .enumerate()
                .filter(|(i, _)| *i != 0)
                .map(|(_, t)| t)
                .collect::<Vec<&str>>();

            Some(author_tokens.join(" "))
        })
        .map(|a| a.to_string())
        .collect::<Vec<String>>();

    authors.sort();

    std::env::set_current_dir(old_cwd)?;

    Ok(authors)
}

fn get_num_author_commits(author: &str, target_dir: impl AsRef<Path>) -> eyre::Result<usize> {
    let old_cwd = std::env::current_dir()?;
    std::env::set_current_dir(target_dir.as_ref())?;

    let command = format!("git rev-list HEAD --author='{author}' --count --all");
    let stdout = get_stdout_from_subprocess_or_fail(&command)?;

    let num_of_commits = stdout.parse().with_suggestion(|| {
        format!("Failed to run '{command}'. '--count' didn't return a number!")
    })?;

    std::env::set_current_dir(old_cwd)?;

    Ok(num_of_commits)
}

fn get_num_author_edits(author: &str, target_dir: impl AsRef<Path>) -> eyre::Result<CodeEdit> {
    let old_cwd = std::env::current_dir()?;
    std::env::set_current_dir(target_dir.as_ref())?;

    let command = format!("git log --author='{author}' --numstat --pretty=tformat:");
    let stdout = get_stdout_from_subprocess_or_fail(&command)?;

    let mut total_additions = 0;
    let mut total_removals = 0;

    stdout.lines().for_each(|l| {
        let tokens: Vec<&str> = l.split_ascii_whitespace().collect();

        if let Some(additions) = tokens.get(0) {
            total_additions += additions.parse().unwrap_or(0);
        }
        if let Some(removals) = tokens.get(1) {
            total_removals += removals.parse().unwrap_or(0);
        }
    });

    std::env::set_current_dir(old_cwd)?;

    let ce = CodeEdit {
        additions: total_additions,
        removals: total_removals,
    };

    Ok(ce)
}

fn get_stdout_from_subprocess_or_fail(command: &str) -> eyre::Result<String> {
    use std::process::Command;

    log::debug!("Running '{command}'");
    let subprocess_result = Command::new("bash").args(["-c", command]).output()?;

    if !subprocess_result.status.success() {
        let stderr = String::from_utf8(subprocess_result.stderr).unwrap_or_default();
        log::warn!("stderr from subprocess: {stderr}");
        eyre::bail!("Failed to run '{command}'");
    }

    let mut stdout = String::from_utf8(subprocess_result.stdout).unwrap_or_default();
    if stdout.ends_with('\n') {
        stdout.pop();
    }

    Ok(stdout.to_string())
}
