//! `gh_raw`: any `gh` command, with its outcome, failure included, as a
//! value.

use serde_json::{Value, json};

use super::{Args, Context, Reply};

pub fn run(context: &Context, args: &Args) -> Result<Reply, String> {
    let args: Vec<String> = args
        .get("args")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let output = context.gh.raw(&args);
    Ok(Reply::Json(json!({
        "ok": output.exit_code == 0,
        "exit_code": output.exit_code,
        "stdout": output.stdout,
        "stderr": output.stderr,
    })))
}

#[cfg(test)]
mod tests {
    use crate::mcp::gh::{Gh, Output};
    use crate::mcp::tests::call_json;
    use serde_json::json;

    #[test]
    fn it_returns_the_outcome_of_a_successful_command() {
        let gh = Gh::with(|args| {
            Ok(Output {
                exit_code: 0,
                stdout: args.iter().map(|arg| format!("{arg} ")).collect(),
                stderr: String::new(),
            })
        });

        let outcome = call_json(
            gh,
            "gh_raw",
            json!({"args": ["release", "list", "--repo", "epik-agent/Epik"]}),
        );

        assert_eq!(
            outcome.unwrap(),
            json!({
                "ok": true,
                "exit_code": 0,
                "stdout": "release list --repo epik-agent/Epik ",
                "stderr": "",
            })
        );
    }

    #[test]
    fn it_returns_the_outcome_of_a_failing_command() {
        let gh = Gh::with(|_| {
            Ok(Output {
                exit_code: 1,
                stdout: String::new(),
                stderr: "unknown command \"nope\"\n".to_owned(),
            })
        });

        let outcome = call_json(gh, "gh_raw", json!({"args": ["nope"]}));

        assert_eq!(
            outcome.unwrap(),
            json!({
                "ok": false,
                "exit_code": 1,
                "stdout": "",
                "stderr": "unknown command \"nope\"\n",
            })
        );
    }

    #[test]
    fn it_reports_gh_missing_or_stopped_as_a_failed_command() {
        for message in [
            "gh is not installed. Install it from https://cli.github.com/",
            "gh was stopped by a signal: gh issue list",
        ] {
            let gh = Gh::with(move |_| Err(message.to_owned()));

            let outcome = call_json(gh, "gh_raw", json!({"args": ["issue", "list"]}));

            assert_eq!(
                outcome.unwrap(),
                json!({"ok": false, "exit_code": -1, "stdout": "", "stderr": message})
            );
        }
    }
}
