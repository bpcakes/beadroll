//! G20: the settings that let git reach the remote from the checkout reach bead's fetch and push
//! too: the checkout's transport keys, global includes, `GIT_SSH` and `core.sshCommand`
//! (`docs/FINDINGS.md`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::Permissions;
use std::io::{self, BufRead, BufReader, Read as _, Write as _};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use beadroll_test_support::{Json, Machine, Sandbox};

/// Variables to set, by name.
type Env = BTreeMap<OsString, OsString>;

/// `_g20_config`: a scratch global git config for this sandbox (never the developer's) that says
/// `text`, and the variables that make git read it and no system config.
fn config(sandbox: &Sandbox, text: &str) -> Env {
    let path = sandbox.root().join("global.gitconfig");
    std::fs::write(&path, text).unwrap();
    Env::from([
        ("GIT_CONFIG_GLOBAL".into(), path.into_os_string()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ])
}

/// `_g20_git`: git ARGS as user `t`, in `cwd` (the test's own directory when `None`), with the
/// test's environment less its `GIT_*` variables, then `env`; its output, trimmed. A failure fails
/// the test with git's standard error.
fn git(args: &[&str], cwd: Option<&Path>, env: &Env) -> String {
    let inherited = std::env::vars_os().filter(|(k, _)| !k.as_encoded_bytes().starts_with(b"GIT_"));
    let mut command = Command::new("git");
    command
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .env_clear()
        .envs(inherited)
        .envs(env);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().expect("git starts");
    assert!(
        output.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("git's output is UTF-8");
    stdout.trim().to_owned()
}

/// `path` as the text of an argument or of a config value.
fn utf8(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

/// `_g20_serve`: smart HTTP (`git http-backend`) for the repositories under a root on 127.0.0.1,
/// served as Python's `ThreadingHTTPServer` serves it: HTTP/1.0, one request a connection, each on
/// a thread of its own. Every request must carry the Authorization header `token` exactly once and
/// the X-Guard header exactly once; otherwise 401. Dropping it stops it, as the Python test's
/// cleanups shut its server down.
struct Server {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// Serves the repositories under `root` to requests that carry `token`.
    fn start(root: &Path, token: &str) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port on 127.0.0.1");
        let address = listener.local_addr().expect("the server's address");
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let (root, token) = (root.to_path_buf(), token.to_owned());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = stream else { continue };
                let (root, token) = (root.clone(), token.clone());
                std::thread::spawn(move || {
                    if let Err(error) = answer(stream, &root, &token) {
                        eprintln!("the G20 server failed a request: {error}");
                    }
                });
            }
        });
        Server { address, stop }
    }

    /// The URL its repositories are under: `http://127.0.0.1:PORT`.
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.address.port())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // The server's thread waits in `accept`: a connection wakes it to see that it must stop.
        let _ = TcpStream::connect(self.address);
    }
}

/// `Handler.serve`: reads one request from `stream` and answers it.
fn answer(stream: TcpStream, root: &Path, token: &str) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let mut words = request.split_whitespace();
    let (Some(method), Some(target)) = (words.next(), words.next()) else {
        return Ok(());
    };
    let headers = read_headers(&mut reader)?;
    if all(&headers, "Authorization") != [token] || all(&headers, "X-Guard").len() != 1 {
        writer.write_all(
            b"HTTP/1.0 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"g20\"\r\n\
              Content-Length: 0\r\n\r\n",
        )?;
        return writer.shutdown(Shutdown::Write);
    }
    let first = |name: &str| all(&headers, name).first().copied();
    let body = if first("Transfer-Encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        chunked(&mut reader)?
    } else {
        let length = first("Content-Length").filter(|v| !v.is_empty());
        let length = length.map_or(0, |v| v.trim().parse().expect("a Content-Length"));
        let mut body = vec![0; length];
        reader.read_exact(&mut body)?;
        body
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut cgi = Command::new("git");
    cgi.arg("http-backend")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("the test's PATH"))
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("REMOTE_USER", "ci")
        .env("REQUEST_METHOD", method)
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("CONTENT_TYPE", first("Content-Type").unwrap_or(""))
        .env("CONTENT_LENGTH", body.len().to_string())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    for (header, name) in [
        ("Git-Protocol", "HTTP_GIT_PROTOCOL"),
        ("Content-Encoding", "HTTP_CONTENT_ENCODING"),
    ] {
        if let Some(value) = first(header).filter(|v| !v.is_empty()) {
            cgi.env(name, value);
        }
    }
    let out = run_with_input(cgi, body)?;
    let at = out.windows(4).position(|w| w == b"\r\n\r\n");
    let (head, payload) = at.map_or((&out[..], &[][..]), |at| (&out[..at], &out[at + 4..]));
    let head = std::str::from_utf8(head).expect("http-backend's headers are UTF-8");
    let mut status: u16 = 200;
    let mut response = String::new();
    for line in head.split("\r\n") {
        let (name, value) = line.split_once(':').unwrap_or((line, ""));
        if name.eq_ignore_ascii_case("status") {
            let code = value.split_whitespace().next().and_then(|c| c.parse().ok());
            status = code.expect("a status code");
        } else if !name.is_empty() && !name.eq_ignore_ascii_case("content-length") {
            response.push_str(&format!("{name}: {}\r\n", value.trim()));
        }
    }
    let response = format!(
        "HTTP/1.0 {status} {}\r\n{response}Content-Length: {}\r\n\r\n",
        reason(status),
        payload.len()
    );
    writer.write_all(response.as_bytes())?;
    writer.write_all(payload)?;
    writer.shutdown(Shutdown::Write)
}

/// The header lines of a request, up to the blank line, as Python's `http.client` parses them: the
/// name as it came, the value without the blanks that lead it.
fn read_headers(reader: &mut impl BufRead) -> io::Result<Vec<(String, String)>> {
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(headers);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return Ok(headers);
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim_start_matches([' ', '\t']);
            headers.push((name.to_owned(), value.to_owned()));
        }
    }
}

/// `headers.get_all(name)`: the value of each header `name`, whatever its case, in order.
fn all<'h>(headers: &'h [(String, String)], name: &str) -> Vec<&'h str> {
    let named = headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name));
    named.map(|(_, value)| value.as_str()).collect()
}

/// A body sent in chunks, read as the Python handler reads it.
fn chunked(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let size = usize::from_str_radix(line.trim(), 16).map_err(io::Error::other)?;
        if size == 0 {
            reader.read_line(&mut String::new())?;
            return Ok(body);
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..])?;
        reader.read_line(&mut String::new())?;
    }
}

/// What `command` printed on its standard output, `input` on its standard input.
fn run_with_input(mut command: Command, input: Vec<u8>) -> io::Result<Vec<u8>> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().expect("the child's input");
    // Written from another thread: a child that fills its output before it reads all of its input
    // would otherwise wait for us while we wait for it.
    let feeder = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let output = child.wait_with_output()?;
    let _ = feeder.join();
    Ok(output.stdout)
}

/// The reason phrase Python's `send_response` sends with `status`.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "",
    }
}

/// `_g20_fake_ssh`: a folder `bin` in the sandbox whose `ssh` refuses (nothing leaves the
/// machine) and whose `user-ssh` runs the remote command locally; the `PATH` that puts it first.
fn fake_ssh(sandbox: &Sandbox) -> Env {
    let folder = sandbox.root().join("bin");
    std::fs::create_dir_all(&folder).unwrap();
    let refusing = "#!/bin/sh\necho 'plain ssh: blocked in this test' >&2\nexit 255\n";
    std::fs::write(folder.join("ssh"), refusing).unwrap();
    let local = "#!/bin/sh\nfor last; do :; done\nexec sh -c \"$last\"\n";
    std::fs::write(folder.join("user-ssh"), local).unwrap();
    for entry in std::fs::read_dir(&folder).unwrap() {
        let path = entry.unwrap().path();
        std::fs::set_permissions(path, Permissions::from_mode(0o755)).unwrap();
    }
    let mut path = folder.into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").expect("the test's PATH"));
    Env::from([("PATH".into(), path)])
}

/// `assert_bead_syncs`: in `checkout`, `machine` initialises tracker `tr` and creates an issue;
/// then a sync and a claim of the issue both succeed.
fn assert_bead_syncs(machine: &Machine, checkout: &Path) {
    machine.cmd(["init", "--prefix", "tr"]).cwd(checkout).json();
    let created = machine.cmd(["create", "one"]).cwd(checkout).json();
    let created = created.expect("the new issue");
    let issue = created.get("id").and_then(Json::as_str);
    let issue = issue.expect("the issue's id");
    let sync = machine.cmd(["sync", "--json"]).cwd(checkout).run();
    assert_eq!(
        sync.returncode(),
        0,
        "git reaches the remote from this checkout, bead does not: {}",
        sync.stderr
    );
    let claim = machine.cmd(["claim", issue, "--json"]).cwd(checkout).run();
    assert_eq!(claim.returncode(), 0, "{}", claim.stderr);
}

/// G20: sync.git lives under BEAD_HOME and bead passes it only the remote URL, so the settings
/// that let git reach the remote from the checkout do not apply to bead's fetch and push: the
/// checkout's own config (actions/checkout keeps its token there as http.<url>.extraheader), global
/// includes conditioned on `gitdir:` or `hasconfig:`, and GIT_SSH or core.sshCommand, which bead's
/// own GIT_SSH_COMMAND default overrides. sync and every online claim then fail with
/// REMOTE_UNREACHABLE where git itself works.
mod g20_transport_settings {
    use beadroll_test_support::Machine;

    use super::{Server, assert_bead_syncs, config, fake_ssh, git, utf8};

    /// G20: a token in the checkout's own config (actions/checkout's http.<url>.extraheader)
    /// never reaches bead's fetch; a header the global config already sends to both is still sent
    /// once.
    #[test]
    fn actions_checkout_token_in_the_repository_config() {
        let sandbox = crate::sandbox();
        let token = "Basic eC1hY2Nlc3MtdG9rZW46Z2hzX3Rlc3R0b2tlbg==";
        let server = Server::start(sandbox.root(), token);
        let base = server.base();
        // an include that applies to the checkout and to sync.git
        let guard = sandbox.root().join("guard.gitconfig");
        let header = format!("[http \"{base}/\"]\n\textraheader = X-Guard: 1\n");
        std::fs::write(&guard, header).unwrap();
        let (root, guard) = (utf8(sandbox.root()), utf8(&guard));
        let genv = config(
            &sandbox,
            &format!("[includeIf \"gitdir:{root}/\"]\n\tpath = {guard}\n"),
        );
        let mut env = genv.clone();
        env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
        let checkout = sandbox.root().join("ci");
        let at = Some(checkout.as_path());
        // what actions/checkout does
        git(&["init", "-q", utf8(&checkout)], None, &env);
        let url = format!("{base}/origin.git");
        git(&["remote", "add", "origin", &url], at, &env);
        let (key, value) = (
            format!("http.{base}/.extraheader"),
            format!("AUTHORIZATION: {token}"),
        );
        git(&["config", "--local", &key, &value], at, &env);
        git(&["fetch", "-q", "origin", "main"], at, &env);
        git(&["checkout", "-q", "-b", "main", "FETCH_HEAD"], at, &env);
        let probe = ["push", "-q", "--dry-run", "origin", "HEAD:refs/heads/probe"];
        git(&probe, at, &env);
        let mut m = Machine::new(&sandbox, "ci");
        m.extra.extend(genv);
        assert_bead_syncs(&m, &checkout);
    }

    /// G20: GIT_SSH is overridden by bead's GIT_SSH_COMMAND default.
    #[test]
    fn git_ssh_program() {
        let sandbox = crate::sandbox();
        let genv = config(&sandbox, "");
        let mut a = Machine::new(&sandbox, "a");
        a.extra.extend(genv);
        a.extra.extend(fake_ssh(&sandbox));
        let user_ssh = sandbox.root().join("bin").join("user-ssh");
        a.extra.insert("GIT_SSH".into(), user_ssh.into_os_string());
        a.extra.insert("GIT_SSH_VARIANT".into(), "simple".into());
        let url = format!("ssh://fakehost{}", utf8(sandbox.origin()));
        let env = a.env(None, "codex");
        git(
            &["remote", "set-url", "origin", &url],
            Some(a.checkout()),
            &env,
        );
        // git reaches the remote with GIT_SSH
        git(&["fetch", "-q", "origin"], Some(a.checkout()), &env);
        assert_bead_syncs(&a, a.checkout());
    }

    /// G20: the checkout's core.sshCommand is invisible to sync.git and overridden by bead's
    /// default.
    #[test]
    fn core_ssh_command_in_the_repository_config() {
        let sandbox = crate::sandbox();
        let genv = config(&sandbox, "");
        let mut a = Machine::new(&sandbox, "a");
        a.extra.extend(genv);
        a.extra.extend(fake_ssh(&sandbox));
        a.extra.insert("GIT_SSH_VARIANT".into(), "simple".into());
        let url = format!("ssh://fakehost{}", utf8(sandbox.origin()));
        let env = a.env(None, "codex");
        git(
            &["remote", "set-url", "origin", &url],
            Some(a.checkout()),
            &env,
        );
        let user_ssh = sandbox.root().join("bin").join("user-ssh");
        let set = ["config", "--local", "core.sshCommand", utf8(&user_ssh)];
        git(&set, Some(a.checkout()), &env);
        git(&["fetch", "-q", "origin"], Some(a.checkout()), &env);
        assert_bead_syncs(&a, a.checkout());
    }

    /// G20: a url rewrite in a global include scoped with includeIf "gitdir:" (the usual
    /// multi-account setup) does not apply to sync.git.
    #[test]
    fn include_if_scoped_url_rewrite() {
        let sandbox = crate::sandbox();
        let ssh = fake_ssh(&sandbox);
        let work = sandbox.root().join("work");
        std::fs::create_dir(&work).unwrap();
        let include = sandbox.root().join("work.gitconfig");
        let origin = utf8(sandbox.origin());
        let rewrite = format!("[url \"{origin}\"]\n\tinsteadOf = git@github.com:corp/proj.git\n");
        std::fs::write(&include, rewrite).unwrap();
        let (scope, include) = (utf8(&work), utf8(&include));
        let genv = config(
            &sandbox,
            &format!("[includeIf \"gitdir:{scope}/\"]\n\tpath = {include}\n"),
        );
        let checkout = work.join("proj");
        let mut env = genv.clone();
        env.extend(ssh.clone());
        let clone = [
            "clone",
            "-q",
            "git@github.com:corp/proj.git",
            utf8(&checkout),
        ];
        git(&clone, None, &env);
        let mut m = Machine::new(&sandbox, "w");
        m.extra.extend(genv);
        m.extra.extend(ssh);
        assert_bead_syncs(&m, &checkout);
    }
}
