use std::io::{BufRead, Lines, Write};

use grt_upstream::{PushSpec, PushStatus, RefValue, TransferOptions};

use crate::error::Error;
use crate::remote::{FetchRef, Remote};

const CAPABILITIES: &str = "fetch\npush\noption\n";

/// Serves one remote-helper session: reads git's commands from `input` and
/// writes replies to `output` until git sends a blank line or closes `input`.
///
/// # Errors
///
/// Returns an error if I/O fails, git sends an unknown or malformed command,
/// or the remote fails. Git reports the helper's non-zero exit to the user.
pub fn run<M, R, W>(remote: &mut M, input: R, mut output: W) -> Result<(), Error>
where
    M: Remote,
    R: BufRead,
    W: Write,
{
    let mut lines = input.lines();
    let mut options = TransferOptions::default();
    while let Some(line) = lines.next() {
        let line = line?;
        let (command, argument) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        match command {
            "" => break,
            "capabilities" => writeln!(output, "{CAPABILITIES}")?,
            "option" => writeln!(output, "{}", set_option(&mut options, argument))?,
            "list" => {
                // `list for-push` gets the same answer.
                for listed in remote.list(&options)? {
                    match listed.value {
                        RefValue::Object(id) => writeln!(output, "{id} {}", listed.name)?,
                        RefValue::Symbolic(target) => {
                            writeln!(output, "@{target} {}", listed.name)?;
                        }
                    }
                }
                writeln!(output)?;
            }
            "fetch" => {
                let refs = read_batch(&mut lines, argument, "fetch")?
                    .iter()
                    .map(|argument| parse_fetch(argument))
                    .collect::<Result<Vec<FetchRef>, Error>>()?;
                remote.fetch(&refs, &options)?;
                writeln!(output)?;
            }
            "push" => {
                let specs = read_batch(&mut lines, argument, "push")?
                    .iter()
                    .map(|argument| argument.parse())
                    .collect::<Result<Vec<PushSpec>, grt_upstream::Error>>()?;
                for result in remote.push(&specs, &options)? {
                    match result.status {
                        PushStatus::Ok => writeln!(output, "ok {}", result.dst)?,
                        PushStatus::Rejected(reason) => {
                            writeln!(output, "error {} {reason}", result.dst)?;
                        }
                    }
                }
                writeln!(output)?;
            }
            _ => return Err(Error::Protocol(format!("unknown command {line:?}"))),
        }
        output.flush()?;
    }
    Ok(())
}

/// Collects a batch of `<keyword> <argument>` lines ending in a blank line.
fn read_batch<R: BufRead>(
    lines: &mut Lines<R>,
    first: &str,
    keyword: &str,
) -> Result<Vec<String>, Error> {
    let mut batch = vec![first.to_owned()];
    for line in lines {
        let line = line?;
        if line.is_empty() {
            return Ok(batch);
        }
        let Some(argument) = line
            .strip_prefix(keyword)
            .and_then(|rest| rest.strip_prefix(' '))
        else {
            return Err(Error::Protocol(format!(
                "expected `{keyword}` in batch, got {line:?}"
            )));
        };
        batch.push(argument.to_owned());
    }
    Err(Error::Protocol(format!("`{keyword}` batch not terminated")))
}

/// Parses a `fetch <oid> <name>` argument.
fn parse_fetch(argument: &str) -> Result<FetchRef, Error> {
    let malformed = || Error::Protocol(format!("malformed fetch {argument:?}"));
    let (id, name) = argument.split_once(' ').ok_or_else(malformed)?;
    // The name is passed to `git fetch`; only accept real ref names.
    if name != "HEAD" && !name.starts_with("refs/") {
        return Err(malformed());
    }
    Ok(FetchRef {
        id: id.parse().map_err(|_| malformed())?,
        name: name.to_owned(),
    })
}

/// Applies `option <name> <value>` and returns the reply line.
fn set_option(options: &mut TransferOptions, argument: &str) -> &'static str {
    let (name, value) = argument.split_once(' ').unwrap_or((argument, ""));
    match (name, value) {
        ("verbosity", level) => match level.parse() {
            Ok(level) => {
                options.verbosity = level;
                "ok"
            }
            Err(_) => "error invalid verbosity",
        },
        ("progress", "true") => {
            options.progress = Some(true);
            "ok"
        }
        ("progress", "false") => {
            options.progress = Some(false);
            "ok"
        }
        _ => "unsupported",
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use grt_upstream::{ListedRef, PushResult};

    use super::*;

    const A: &str = "ce013625030ba8dba906f756967f9e9ca394464a";

    /// Records every call and answers from canned data.
    #[derive(Default)]
    struct FakeRemote {
        calls: RefCell<Vec<String>>,
        reject: Option<&'static str>,
    }

    impl Remote for FakeRemote {
        fn list(&mut self, _options: &TransferOptions) -> Result<Vec<ListedRef>, Error> {
            self.calls.borrow_mut().push("list".to_owned());
            Ok(vec![
                ListedRef::symbolic("HEAD", "refs/heads/main"),
                ListedRef::object("refs/heads/main", A.parse().unwrap()),
            ])
        }

        fn fetch(&mut self, refs: &[FetchRef], options: &TransferOptions) -> Result<(), Error> {
            let names: Vec<&str> = refs.iter().map(|wanted| wanted.name.as_str()).collect();
            self.calls
                .borrow_mut()
                .push(format!("fetch {names:?} {options:?}"));
            Ok(())
        }

        fn push(
            &mut self,
            specs: &[PushSpec],
            _options: &TransferOptions,
        ) -> Result<Vec<PushResult>, Error> {
            self.calls.borrow_mut().push(format!("push {specs:?}"));
            Ok(specs
                .iter()
                .map(|spec| PushResult {
                    dst: spec.dst.clone(),
                    status: match self.reject {
                        Some(dst) if dst == spec.dst => {
                            PushStatus::Rejected("non-fast-forward".to_owned())
                        }
                        _ => PushStatus::Ok,
                    },
                })
                .collect())
        }
    }

    fn transcript(remote: &mut FakeRemote, input: &str) -> String {
        let mut output = Vec::new();
        run(remote, input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn clone_session() {
        let mut remote = FakeRemote::default();
        let output = transcript(
            &mut remote,
            &format!(
                "capabilities\noption progress true\noption depth 1\nlist\n\
                 fetch {A} HEAD\nfetch {A} refs/heads/main\n\n\n"
            ),
        );
        assert_eq!(
            output,
            format!(
                "fetch\npush\noption\n\nok\nunsupported\n@refs/heads/main HEAD\n{A} refs/heads/main\n\n\n"
            ),
            "capabilities, option replies, ref list and fetch terminator"
        );
        assert_eq!(
            *remote.calls.borrow(),
            vec![
                "list".to_owned(),
                r#"fetch ["HEAD", "refs/heads/main"] TransferOptions { verbosity: 1, progress: Some(true) }"#.to_owned(),
            ],
            "one batched fetch carrying the negotiated options"
        );
    }

    #[test]
    fn push_session_reports_each_ref() {
        let mut remote = FakeRemote {
            reject: Some("refs/heads/old"),
            ..FakeRemote::default()
        };
        let output = transcript(
            &mut remote,
            "list for-push\npush refs/heads/main:refs/heads/main\npush +refs/heads/x:refs/heads/old\n\n",
        );
        assert_eq!(
            output,
            format!(
                "@refs/heads/main HEAD\n{A} refs/heads/main\n\n\
                 ok refs/heads/main\nerror refs/heads/old non-fast-forward\n\n"
            ),
            "per-ref ok/error lines then a blank line"
        );
    }

    #[test]
    fn eof_ends_session_cleanly() {
        assert_eq!(
            transcript(&mut FakeRemote::default(), "capabilities\n"),
            "fetch\npush\noption\n\n"
        );
    }

    #[test]
    fn rejects_bad_input() {
        for input in [
            "frobnicate\n",
            &format!("fetch {A} --upload-pack=evil\n\n"),
            "fetch not-an-id refs/heads/main\n\n",
            &format!("fetch {A} refs/heads/main\nlist\n"),
            &format!("fetch {A} refs/heads/main\n"),
            "push --mirror:refs/heads/x\n\n",
        ] {
            let mut output = Vec::new();
            assert!(
                run(&mut FakeRemote::default(), input.as_bytes(), &mut output).is_err(),
                "{input:?} must end the session with an error"
            );
        }
    }
}
