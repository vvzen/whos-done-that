use std::fmt::Display;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Parser;
use color_eyre::{eyre, Section};
use shell_quote::Bash;
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(
    version,
    long_about = "A CLI to help establish ownernship of codebases"
)]
struct Cli {
    #[arg(
        short,
        long,
        help = "The target directory to analyze. It must be a git repo. If not provided, the current directory will be used instead."
    )]
    target_dir: Option<PathBuf>,

    #[arg(
        short,
        long,
        help = "Branch name used to search for commit authors.",
        default_value = "main"
    )]
    branch: String,
}

struct CodeEdits {
    additions: usize,
    removals: usize,
}

struct AuthorData {
    author_name: String,
    code_edits: CodeEdits,
    num_commits: usize,
}

impl Display for CodeEdits {
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
        .with_target(false)
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::INFO)
        .without_time()
        .finish();

    tracing::subscriber::set_global_default(subscriber)?;

    let target_dir = cli.target_dir.unwrap_or(cwd);

    tracing::info!("Getting a list of authors..");
    let authors = get_all_authors(&target_dir, &cli.branch)?;

    let mut authors_data = Vec::new();

    tracing::info!("Compiling stats..");

    // TODO: This could be parallelized with rayon
    for author in authors {
        let num_commits = get_num_author_commits(&author, target_dir.as_path(), &cli.branch)?;
        let code_edits = get_num_author_edits(&author, target_dir.as_path(), &cli.branch)?;
        authors_data.push(AuthorData {
            num_commits,
            code_edits,
            author_name: author.to_owned(),
        });
    }
    authors_data.sort_by(|a, b| a.num_commits.cmp(&b.num_commits).reverse());

    // Print the final stats
    let stdout = std::io::stdout();
    let mut stdout_handle = std::io::BufWriter::new(stdout);

    for author_data in authors_data {
        let ending = match author_data.num_commits {
            0 => {
                continue;
            }
            1 => "1 commit".to_string(),
            _ => format!("{} commits", author_data.num_commits),
        };

        writeln!(
            stdout_handle,
            "{} has made {ending}: {}",
            author_data.author_name, author_data.code_edits
        )?;
    }

    Ok(())
}

/// Return a list sorted lexicographically (by byte values) containing
/// all of the detected authors for the git repository living at `target_dir`.
/// For more notes on the sorting, see:
/// https://doc.rust-lang.org/std/primitive.str.html#impl-Ord
fn get_all_authors(target_dir: impl AsRef<Path>, branch_name: &str) -> eyre::Result<Vec<String>> {
    let b = String::from_utf8(Bash::quote_vec(branch_name))?;
    let command = format!(
        "git -C {} shortlog --summary --numbered --no-merges --all --branches={b}",
        target_dir.as_ref().display()
    );
    let stdout = get_stdout_from_subprocess_or_fail(&command)?;

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

    Ok(authors)
}

/// Return the number of commits authored by the given `author`
/// for the git repository living at `target_dir`.
fn get_num_author_commits(
    author: &str,
    target_dir: impl AsRef<Path>,
    branch_name: &str,
) -> eyre::Result<usize> {
    let a = String::from_utf8(Bash::quote_vec(author))?;
    let b = String::from_utf8(Bash::quote_vec(branch_name))?;

    let command = format!(
        "git -C {} rev-list HEAD --author={a} --count --branches={b}",
        target_dir.as_ref().display(),
    );
    let stdout = get_stdout_from_subprocess_or_fail(&command)?;

    let num_of_commits = stdout.parse().with_suggestion(|| {
        format!("Failed to run '{command}'. '--count' didn't return a number!")
    })?;

    Ok(num_of_commits)
}

/// Return the number of edits authored by the given `author`
/// for the git repository living at `target_dir`.
fn get_num_author_edits(
    author: &str,
    target_dir: impl AsRef<Path>,
    branch_name: &str,
) -> eyre::Result<CodeEdits> {
    let a = String::from_utf8(Bash::quote_vec(author))?;
    let b = String::from_utf8(Bash::quote_vec(branch_name))?;

    let command = format!(
        "git -C {} log --author={a} --numstat --pretty=tformat: --branches={b} --all",
        target_dir.as_ref().display()
    );
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

    let ce = CodeEdits {
        additions: total_additions,
        removals: total_removals,
    };

    Ok(ce)
}

/// Run the given `command` in a bash subshell and return back `stdout` if
/// it exited with 0. If the command exited with non 0 this will return an error
/// and prints `stderr`.
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
