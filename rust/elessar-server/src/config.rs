//! Command line, `.env`, and where the repo is.

use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 8778;
const USAGE: &str = "usage: elessar-server [--scenario ID] [--scenarios-dir DIR] [--host HOST] [--port PORT] [--check]";

/// `KEY=VALUE` lines as `load_dotenv` in `server/app.py` reads them: blank
/// lines and `#` comments skipped, split at the first `=`, the key trimmed,
/// the value trimmed and stripped of surrounding quotes.
pub fn parse_dotenv(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_string(), value.trim().trim_matches(['"', '\'']).to_string()))
        })
        .collect()
}

/// Load `.env` into the process environment; real environment variables win.
/// A missing or unreadable file is no error. Call it before starting threads.
pub fn load_dotenv(path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    for (key, value) in parse_dotenv(&text) {
        if !key.is_empty() && !key.contains('\0') && std::env::var_os(&key).is_none() {
            std::env::set_var(key, value);
        }
    }
}

fn is_repo(dir: &Path) -> bool {
    dir.join("web").is_dir() && dir.join("scenarios").is_dir()
}

/// The repo holding `web/` and `scenarios/`: found upward from the working
/// directory, else upward from the binary, else where this crate was built.
pub fn find_root() -> PathBuf {
    let cwd = std::env::current_dir().ok();
    let exe = std::env::current_exe().ok();
    let starts = cwd.as_deref().into_iter().chain(exe.as_deref());
    for start in starts {
        if let Some(found) = start.ancestors().find(|d| is_repo(d)) {
            return found.to_path_buf();
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap_or(Path::new(".")).to_path_buf()
}

/// The command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Args {
    pub scenario: String,
    pub scenarios_dir: PathBuf,
    pub host: String,
    pub port: u16,
    pub check: bool,
}

impl Args {
    /// Accepts `--flag value` and `--flag=value`. `Err` carries the usage line.
    pub fn parse(args: impl IntoIterator<Item = String>, root: &Path) -> Result<Args, String> {
        let mut out = Args { scenario: "change-cascade".into(), scenarios_dir: root.join("scenarios"), host: "127.0.0.1".into(), port: DEFAULT_PORT, check: false };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
                _ => (arg, None),
            };
            if flag == "--check" {
                out.check = true;
                continue;
            }
            if !matches!(flag.as_str(), "--scenario" | "--scenarios-dir" | "--host" | "--port") {
                return Err(format!("unknown argument '{flag}'\n{USAGE}"));
            }
            let value = inline.or_else(|| args.next()).ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?;
            match flag.as_str() {
                "--scenario" => out.scenario = value,
                "--scenarios-dir" => out.scenarios_dir = PathBuf::from(value),
                "--host" => out.host = value,
                _ => out.port = value.parse().map_err(|_| format!("--port needs a number, got '{value}'\n{USAGE}"))?,
            }
        }
        Ok(out)
    }
}

/// Everything the app reads from the outside world, so tests can set it.
#[derive(Debug, Clone)]
pub struct Config {
    pub scenarios_dir: PathBuf,
    pub web_dir: PathBuf,
    pub jev_cache_dir: PathBuf,
    pub default_scenario: String,
    /// `None` when `OPENROUTER_API_KEY` is unset or empty: Jev and the agent are off.
    pub api_key: Option<String>,
    pub decision_model: String,
    pub inference_model: String,
    pub jev_endpoint: String,
    pub messages_endpoint: String,
}

impl Config {
    /// From the command line and the environment, as `.env` has filled it.
    pub fn from_env(root: &Path, args: &Args) -> Config {
        Config {
            scenarios_dir: args.scenarios_dir.clone(),
            web_dir: root.join("web"),
            jev_cache_dir: root.join(".cache").join("jev"),
            default_scenario: args.scenario.clone(),
            api_key: std::env::var("OPENROUTER_API_KEY").ok().filter(|k| !k.is_empty()),
            decision_model: std::env::var("ELESSAR_DECISION_MODEL").unwrap_or_else(|_| "jev-latest".into()),
            inference_model: std::env::var("ELESSAR_INFERENCE_MODEL").unwrap_or_else(|_| "anthropic/claude-sonnet-5.5".into()),
            jev_endpoint: "https://openrouter.ai/api/v1/systemone".into(),
            messages_endpoint: "https://openrouter.ai/api/v1/messages".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn dotenv_reads_like_the_python_loader() {
        let text = "# a comment\n\nA=1\n  B = two words  \nC=\"quoted\"\nD='single'\nE=a=b\nno equals here\nF=\n  # indented comment\nG=\"'mixed'\"\n";
        let want = [("A", "1"), ("B", "two words"), ("C", "quoted"), ("D", "single"), ("E", "a=b"), ("F", ""), ("G", "mixed")];
        let got = parse_dotenv(text);
        assert_eq!(got, want.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>());
    }

    #[test]
    fn dotenv_never_overrides_the_real_environment() {
        let dir = std::env::temp_dir().join(format!("elessar-dotenv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        std::fs::write(&path, "ELESSAR_TEST_SET=from-file\nELESSAR_TEST_UNSET=from-file\n").unwrap();
        std::env::set_var("ELESSAR_TEST_SET", "real");
        std::env::remove_var("ELESSAR_TEST_UNSET");
        load_dotenv(&path);
        assert_eq!(std::env::var("ELESSAR_TEST_SET").unwrap(), "real");
        assert_eq!(std::env::var("ELESSAR_TEST_UNSET").unwrap(), "from-file");
        load_dotenv(&dir.join("missing"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn args_take_both_spellings_and_default_to_the_rust_port() {
        let root = Path::new("/repo");
        let a = Args::parse(strings(&[]), root).unwrap();
        assert_eq!((a.port, a.scenario.as_str(), a.host.as_str(), a.check), (8778, "change-cascade", "127.0.0.1", false));
        assert_eq!(a.scenarios_dir, Path::new("/repo/scenarios"));
        let a = Args::parse(strings(&["--port=9000", "--scenario", "fraud-ring", "--check", "--scenarios-dir", "/x"]), root).unwrap();
        assert_eq!((a.port, a.scenario.as_str(), a.check), (9000, "fraud-ring", true));
        assert_eq!(a.scenarios_dir, Path::new("/x"));
        assert!(Args::parse(strings(&["--port", "nine"]), root).is_err());
        assert!(Args::parse(strings(&["--port"]), root).is_err());
        assert!(Args::parse(strings(&["--nope"]), root).is_err());
    }
}
