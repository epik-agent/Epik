//! `epik-app setup`, the setup checklist for a machine with no window to
//! show it in, such as a host reached over SSH.

use std::path::Path;
use std::process::ExitCode;

use epik_core::install::{Cached, Sources};
use epik_core::record;
use epik_core::setup::{self, Action, Check, Item, State, Step};

pub const USAGE: &str = "usage: epik-app setup [install <item>]";

/// Runs `epik-app setup` with the arguments after `setup`.
pub fn run(args: &[String]) -> ExitCode {
    let Some(root) = record::default_root() else {
        eprintln!("epik-app: there is no home directory to keep Epik's tools in");
        return ExitCode::FAILURE;
    };
    match args {
        [] => list(&root),
        [install, id] if install == "install" => self::install(&root, id),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn checks(root: &Path) -> Vec<Check> {
    let latest = Cached {
        root: root.to_owned(),
        sources: Sources::from_env(),
    };
    setup::check_all(root, &setup::System, &latest)
}

/// Prints each item, its state and detail, and for each not done what to
/// run. Succeeds when a build could start.
fn list(root: &Path) -> ExitCode {
    let checks = checks(root);
    let width = checks
        .iter()
        .map(|check| check.title.len())
        .max()
        .unwrap_or(0);
    for check in &checks {
        let state = match check.state {
            State::Done => "done",
            State::Missing => "missing",
            State::Outdated => "outdated",
        };
        println!("{:width$}  {state:8}  {}", check.title, check.detail);
        for command in setup::step(check)
            .map(|step| commands(root, &step))
            .unwrap_or_default()
        {
            println!("    run: {command}");
        }
    }
    if setup::refusal(&checks).is_none() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Installs Epik's private copy of the item `id`, checked against its
/// published checksum.
fn install(root: &Path, id: &str) -> ExitCode {
    let installable: Vec<&str> = Item::ALL
        .into_iter()
        .filter(|item| item.installable())
        .map(Item::id)
        .collect();
    let Some(item) = Item::from_id(id) else {
        eprintln!(
            "epik-app setup: there is no item {id}; Epik installs {}",
            installable.join(", ")
        );
        return ExitCode::FAILURE;
    };
    if !item.installable() {
        let check = setup::check(root, &setup::System, &setup::Unknown, item);
        match setup::step(&check) {
            Some(step) => eprintln!(
                "epik-app setup: Epik doesn't install {id}; run:\n{}",
                commands(root, &step)
                    .iter()
                    .map(|command| format!("    {command}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            None => eprintln!(
                "epik-app setup: Epik doesn't install {id}, and it is done: {}",
                check.detail
            ),
        }
        return ExitCode::FAILURE;
    }
    let step = Step {
        label: item.title().to_owned(),
        action: Action::Install(item),
    };
    match setup::run_step(root, &step, &|line| println!("{line}")) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("epik-app setup: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The commands to type for `step`, each on a line of its own.
fn commands(root: &Path, step: &Step) -> Vec<String> {
    match &step.action {
        Action::Install(item) => vec![format!("epik-app setup install {}", item.id())],
        Action::Run(commands) => commands
            .iter()
            .map(|words| {
                let program = setup::program(root, &words[0]).display().to_string();
                std::iter::once(program.as_str())
                    .chain(words[1..].iter().map(String::as_str))
                    .map(quoted)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect(),
    }
}

/// `word` as a POSIX shell reads it: single-quoted when it holds anything
/// but letters, digits and `-_./:@=+,`.
fn quoted(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@=+,".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_with_spaces_or_pipes_is_quoted() {
        assert_eq!(quoted("github.com"), "github.com");
        assert_eq!(
            quoted("curl -fsSL https://claude.ai/install.sh | bash"),
            "'curl -fsSL https://claude.ai/install.sh | bash'"
        );
        assert_eq!(quoted("it's"), r"'it'\''s'");
        assert_eq!(quoted(""), "''");
    }

    #[test]
    fn a_step_names_epiks_private_gh_where_there_is_one() {
        let root = tempfile::tempdir().unwrap();
        let step = Step {
            label: "Let git push with gh".to_owned(),
            action: Action::Run(vec![vec!["gh".to_owned(), "auth".to_owned()]]),
        };
        assert_eq!(commands(root.path(), &step), ["gh auth"]);

        let private = setup::private_gh(root.path());
        std::fs::create_dir_all(private.parent().unwrap()).unwrap();
        std::fs::write(&private, "").unwrap();

        assert_eq!(
            commands(root.path(), &step),
            [format!("{} auth", quoted(&private.display().to_string()))]
        );
    }
}
