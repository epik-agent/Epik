//! The `gh` CLI, as Epik's MCP tools run it.
//!
//! Every tool's GitHub call goes through [`Gh`], which runs `gh` and keeps
//! its exit code and output. Tests give it a scripted `gh` instead:
//! [`Gh::scripted`] answers each call with the first reply whose needle its
//! arguments contain.

use std::process::Command;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

/// What one run of `gh` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `gh` with some arguments. An error is a `gh` that never gave an
/// exit code: not installed, or stopped by a signal.
type Run = dyn Fn(&[String]) -> Result<Output, String> + Send + Sync;

/// The `gh` CLI.
#[derive(Clone)]
pub struct Gh {
    run: Arc<Run>,
}

impl Gh {
    /// The `gh` that [`crate::github::gh_program`] finds.
    pub fn new() -> Self {
        Self {
            run: Arc::new(run_gh),
        }
    }

    /// A `gh` that answers with the first of `replies`, each a needle and a
    /// JSON body, whose needle the call's arguments, joined by spaces,
    /// contain; and fails naming the call when none does. Every call is
    /// recorded in `calls`.
    pub fn scripted(
        replies: Vec<(String, Value)>,
        calls: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    ) -> Self {
        Self {
            run: Arc::new(move |args: &[String]| {
                calls.lock().unwrap().push(args.to_vec());
                let call = args.join(" ");
                Ok(
                    match replies.iter().find(|(needle, _)| call.contains(needle)) {
                        Some((_, body)) => Output {
                            exit_code: 0,
                            stdout: format!("{body}\n"),
                            stderr: String::new(),
                        },
                        None => Output {
                            exit_code: 1,
                            stdout: String::new(),
                            stderr: format!("fake gh has no reply for: gh {call}\n"),
                        },
                    },
                )
            }),
        }
    }

    /// A `gh` that runs `run` for each call.
    pub fn with(run: impl Fn(&[String]) -> Result<Output, String> + Send + Sync + 'static) -> Self {
        Self { run: Arc::new(run) }
    }

    /// Runs `gh` and reports what it did, failure included.
    pub fn raw(&self, args: &[String]) -> Output {
        (self.run)(args).unwrap_or_else(|message| Output {
            exit_code: -1,
            stdout: String::new(),
            stderr: message,
        })
    }

    /// Runs `gh` and returns its stdout, or, when it fails, its stderr and
    /// exit code.
    pub fn text(&self, args: &[String]) -> Result<String, String> {
        let output = (self.run)(args)?;
        if output.exit_code == 0 {
            Ok(output.stdout)
        } else {
            Err(explain(output.exit_code, &output.stderr))
        }
    }

    /// Runs `gh` and parses its stdout as JSON.
    pub fn json<T: DeserializeOwned>(&self, args: &[String]) -> Result<T, String> {
        let text = self.text(args)?;
        serde_json::from_str(&text).map_err(|error| format!("unexpected gh output: {error}"))
    }

    /// The JSON at a REST `path`.
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        self.json(&strings(["api", path]))
    }

    /// Every item of the paginated REST list at `path`. Endpoints that wrap
    /// their list in an object name its key as `field`.
    pub fn list(&self, path: &str, field: Option<&str>) -> Result<Vec<Value>, String> {
        let separator = if path.contains('?') { '&' } else { '?' };
        let path = format!("{path}{separator}per_page=100");
        let pages: Vec<Value> = self.json(&strings(["api", "--paginate", "--slurp", &path]))?;
        Ok(pages
            .into_iter()
            .flat_map(|page| {
                let items = match field {
                    Some(field) => page.get(field).cloned().unwrap_or(Value::Null),
                    None => page,
                };
                match items {
                    Value::Array(items) => items,
                    _ => Vec::new(),
                }
            })
            .collect())
    }

    /// Runs a GraphQL `query` and returns its `data`. Each variable is a
    /// field of its own: `-F` for integers, which `gh` sends as numbers,
    /// `-f` for strings.
    pub fn graphql(&self, query: &str, variables: &[(&str, Value)]) -> Result<Value, String> {
        let mut args = strings(["api", "graphql", "-f"]);
        args.push(format!("query={query}"));
        for (name, value) in variables {
            match value {
                Value::Null => {}
                Value::Number(number) => {
                    args.push("-F".to_owned());
                    args.push(format!("{name}={number}"));
                }
                Value::String(text) => {
                    args.push("-f".to_owned());
                    args.push(format!("{name}={text}"));
                }
                other => {
                    args.push("-f".to_owned());
                    args.push(format!("{name}={other}"));
                }
            }
        }
        let reply: Value = self.json(&args)?;
        Ok(reply.get("data").cloned().unwrap_or(Value::Null))
    }
}

impl Default for Gh {
    fn default() -> Self {
        Self::new()
    }
}

/// Owned arguments from string slices.
pub fn strings<const N: usize>(args: [&str; N]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// A failed `gh`'s message: its stderr and exit code, first saying how to
/// sign in when it isn't.
fn explain(exit_code: i32, stderr: &str) -> String {
    let signed_out = if stderr.contains("gh auth login") {
        "gh is not signed in. Run `gh auth login`.\n"
    } else {
        ""
    };
    format!("{signed_out}{}\n(exit code {exit_code})", stderr.trim())
}

fn run_gh(args: &[String]) -> Result<Output, String> {
    let output = Command::new(crate::github::gh_program())
        .args(args)
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                "gh is not installed. Install it from https://cli.github.com/".to_owned()
            }
            _ => format!("gh could not start: {error}"),
        })?;
    let exit_code = output
        .status
        .code()
        .ok_or_else(|| format!("gh was stopped by a signal: gh {}", args.join(" ")))?;
    Ok(Output {
        exit_code,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    fn scripted(replies: Vec<(&str, Value)>) -> (Gh, Arc<Mutex<Vec<Vec<String>>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let replies = replies
            .into_iter()
            .map(|(needle, body)| (needle.to_owned(), body))
            .collect();
        (Gh::scripted(replies, calls.clone()), calls)
    }

    #[test]
    fn a_list_flattens_its_pages() {
        let (gh, calls) = scripted(vec![("repos/o/r/labels", json!([[1, 2], [3]]))]);

        assert_eq!(
            gh.list("repos/o/r/labels", None).unwrap(),
            [json!(1), json!(2), json!(3)]
        );
        assert_eq!(
            calls.lock().unwrap()[0],
            strings([
                "api",
                "--paginate",
                "--slurp",
                "repos/o/r/labels?per_page=100"
            ])
        );
    }

    #[test]
    fn a_wrapped_list_is_read_from_its_field() {
        let (gh, _) = scripted(vec![(
            "check-runs",
            json!([{"check_runs": [1]}, {"check_runs": [2]}]),
        )]);

        let runs = gh.list("repos/o/r/commits/x/check-runs?a=b", Some("check_runs"));

        assert_eq!(runs.unwrap(), [json!(1), json!(2)]);
    }

    #[test]
    fn a_failure_carries_stderr_and_the_exit_code() {
        let (gh, _) = scripted(vec![]);

        let error = gh.text(&strings(["api", "x"])).unwrap_err();

        assert_eq!(error, "fake gh has no reply for: gh api x\n(exit code 1)");
    }

    #[test]
    fn a_signed_out_gh_says_how_to_sign_in() {
        let gh = Gh::with(|_| {
            Ok(Output {
                exit_code: 4,
                stdout: String::new(),
                stderr: "To get started with GitHub CLI, please run:  gh auth login\n".to_owned(),
            })
        });

        let error = gh.text(&strings(["api", "x"])).unwrap_err();

        assert!(
            error.starts_with("gh is not signed in. Run `gh auth login`.\n"),
            "{error}"
        );
        assert!(error.ends_with("(exit code 4)"), "{error}");
    }

    #[test]
    fn graphql_sends_each_variable_as_its_own_field() {
        let (gh, calls) = scripted(vec![("graphql", json!({"data": {"ok": true}}))]);

        let data = gh
            .graphql(
                "query Q",
                &[("owner", json!("o")), ("n", json!(3)), ("x", Value::Null)],
            )
            .unwrap();

        assert_eq!(data, json!({"ok": true}));
        assert_eq!(
            calls.lock().unwrap()[0],
            strings([
                "api",
                "graphql",
                "-f",
                "query=query Q",
                "-f",
                "owner=o",
                "-F",
                "n=3"
            ])
        );
    }
}
