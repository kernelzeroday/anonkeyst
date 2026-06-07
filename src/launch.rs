use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static LITELLM_PID: AtomicU32 = AtomicU32::new(0);

fn install_cleanup_handler() {
    unsafe {
        libc::signal(libc::SIGINT, cleanup_on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, cleanup_on_signal as *const () as libc::sighandler_t);
    }
}

extern "C" fn cleanup_on_signal(_sig: libc::c_int) {
    let pid = LITELLM_PID.load(Ordering::Relaxed);
    if pid != 0 {
        unsafe { libc::kill(pid as i32, libc::SIGTERM); }
    }
    unsafe { libc::_exit(130); }
}

const BASE_URL: &str = "https://anonkey.st/v1";

pub const TOOLS: &[&str] = &["codex", "claude", "aider", "goose", "opencode", "copilot"];

pub fn run(tool: &str, api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    match tool {
        "codex" => launch_codex(api_key, model, extra_args),
        "claude" => launch_claude(api_key, model, extra_args),
        "aider" => launch_aider(api_key, model, extra_args),
        "goose" => launch_goose(api_key, model, extra_args),
        "opencode" => launch_opencode(api_key, model, extra_args),
        "copilot" => launch_copilot(api_key, model, extra_args),
        _ => Err(format!(
            "unknown tool '{}'. supported: {}",
            tool,
            TOOLS.join(", ")
        ).into()),
    }
}

fn find_binary(names: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    for name in names {
        if let Ok(path) = which::which(name) {
            return Ok(path.to_string_lossy().to_string());
        }
    }
    Err(format!(
        "'{}' not found in PATH. Install it first.",
        names[0]
    ).into())
}

const CODEX_PROXY_SCRIPT: &str = include_str!("codex_proxy.py");

fn write_codex_profile(port: u16, model: &str) -> Result<(), Box<dyn std::error::Error>> {
    let codex_home = dirs::home_dir()
        .ok_or("could not determine home directory")?
        .join(".codex");
    std::fs::create_dir_all(&codex_home)?;
    let profile_path = codex_home.join("anonkey.config.toml");

    let config = format!(
r#"model = "{model}"
model_provider = "anonkey"
model_reasoning_effort = "high"

[model_providers.anonkey]
name = "AnonKey"
base_url = "http://127.0.0.1:{port}"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
"#,
        model = model,
        port = port,
    );

    std::fs::write(&profile_path, &config)?;
    Ok(())
}

fn write_codex_auth(api_key: &str) -> Result<String, Box<dyn std::error::Error>> {
    let codex_home = dirs::home_dir()
        .ok_or("could not determine home directory")?
        .join(".codex");
    let auth_path = codex_home.join("auth.json");

    let backup = if auth_path.exists() {
        let content = std::fs::read_to_string(&auth_path)?;
        Some(content)
    } else {
        None
    };

    let auth = format!(r#"{{"auth_mode":"apikey","OPENAI_API_KEY":"{}"}}"#, api_key);
    std::fs::write(&auth_path, &auth)?;

    if let Some(bak) = backup {
        let bak_path = codex_home.join("auth.json.anonkey-bak");
        std::fs::write(&bak_path, &bak)?;
    }

    Ok(auth_path.to_string_lossy().to_string())
}

fn restore_codex_auth() {
    if let Some(home) = dirs::home_dir() {
        let bak = home.join(".codex/auth.json.anonkey-bak");
        let auth = home.join(".codex/auth.json");
        if bak.exists() {
            std::fs::rename(&bak, &auth).ok();
        }
    }
}

fn launch_codex(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let codex_bin = find_binary(&["codex"])?;
    find_binary(&["python3"])?;

    let port = find_free_port()?;
    write_codex_profile(port, model)?;
    write_codex_auth(api_key)?;

    let proxy_dir = std::env::temp_dir().join("anonkeyst");
    std::fs::create_dir_all(&proxy_dir)?;
    let proxy_script = proxy_dir.join(format!("codex_proxy_{}.py", std::process::id()));
    std::fs::write(&proxy_script, CODEX_PROXY_SCRIPT)?;

    eprintln!("starting anonkey proxy on port {}...", port);

    let proxy = Command::new("python3")
        .arg(&proxy_script)
        .env("ANONKEY_API_KEY", api_key)
        .env("ANONKEY_PROXY_PORT", port.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut guard = ProxyGuard {
        child: Some(proxy),
        script_path: proxy_script.to_string_lossy().to_string(),
    };

    LITELLM_PID.store(guard.child.as_ref().unwrap().id(), Ordering::Relaxed);
    install_cleanup_handler();

    // Wait for proxy to be ready
    let start = Instant::now();
    let timeout = Duration::from_secs(10);
    let stderr = guard.child.as_mut().unwrap().stderr.take();
    let mut ready = false;
    if let Some(stderr) = stderr {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            if start.elapsed() > timeout { break; }
            if let Ok(line) = line {
                if line.contains("ready on port") {
                    ready = true;
                    break;
                }
            }
        }
    }
    if !ready {
        // fallback port poll
        let poll_start = Instant::now();
        while poll_start.elapsed() < Duration::from_secs(5) {
            if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if !ready {
        restore_codex_auth();
        return Err("anonkey proxy failed to start".into());
    }

    eprintln!("proxy ready, launching codex...");

    let mut codex = Command::new(&codex_bin)
        .arg("--profile").arg("anonkey")
        .args(extra_args)
        .env("OPENAI_API_KEY", api_key)
        .env_remove("OPENAI_BASE_URL")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    let status = codex.wait()?;
    drop(guard);
    restore_codex_auth();

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

const CLAUDE_PROXY_SCRIPT: &str = include_str!("claude_proxy.py");

struct ProxyGuard {
    child: Option<std::process::Child>,
    script_path: String,
}

impl Drop for ProxyGuard {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            child.kill().ok();
            child.wait().ok();
        }
        std::fs::remove_file(&self.script_path).ok();
    }
}

fn launch_claude(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let claude_bin = find_binary(&["claude"])?;
    find_binary(&["python3"])?;

    let port = find_free_port()?;
    let proxy_url = format!("http://127.0.0.1:{}", port);

    let proxy_dir = std::env::temp_dir().join("anonkeyst");
    std::fs::create_dir_all(&proxy_dir)?;
    let proxy_script = proxy_dir.join(format!("claude_proxy_{}.py", std::process::id()));
    std::fs::write(&proxy_script, CLAUDE_PROXY_SCRIPT)?;

    eprintln!("starting anthropic proxy on port {}...", port);

    let proxy = Command::new("python3")
        .arg(&proxy_script)
        .env("ANONKEY_API_KEY", api_key)
        .env("ANONKEY_MODEL", model)
        .env("ANONKEY_PROXY_PORT", port.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut guard = ProxyGuard {
        child: Some(proxy),
        script_path: proxy_script.to_string_lossy().to_string(),
    };

    LITELLM_PID.store(guard.child.as_ref().unwrap().id(), Ordering::Relaxed);
    install_cleanup_handler();

    // Wait for proxy ready
    let start = Instant::now();
    let timeout = Duration::from_secs(10);
    let mut ready = false;
    if let Some(stderr) = guard.child.as_mut().unwrap().stderr.take() {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            if start.elapsed() > timeout { break; }
            if let Ok(line) = line {
                if line.contains("ready on port") {
                    ready = true;
                    break;
                }
            }
        }
    }
    if !ready {
        let poll_start = Instant::now();
        while poll_start.elapsed() < Duration::from_secs(5) {
            if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if !ready {
        return Err("claude proxy failed to start (requires python3)".into());
    }

    eprintln!("proxy ready, launching claude...");

    let mut claude_cmd = Command::new(&claude_bin);
    claude_cmd.arg("--bare");
    claude_cmd.arg("--dangerously-skip-permissions");
    claude_cmd.args(extra_args);
    claude_cmd.env("ANTHROPIC_BASE_URL", &proxy_url);
    claude_cmd.env("ANTHROPIC_API_KEY", "sk-ant-anonkey-proxy-000000000000000000000000000000000000000000000000");
    // Strip all conflicting env vars
    for key in std::env::vars().filter_map(|(k, _)| {
        if k.starts_with("ANTHROPIC_") || k.starts_with("CLAUDE") || k.starts_with("OPENAI_") || k == "AI_AGENT" {
            Some(k)
        } else {
            None
        }
    }).collect::<Vec<_>>() {
        claude_cmd.env_remove(&key);
    }
    // Re-set the ones we need after clearing
    claude_cmd.env("ANTHROPIC_BASE_URL", &proxy_url);
    claude_cmd.env("ANTHROPIC_API_KEY", "sk-ant-anonkey-proxy-000000000000000000000000000000000000000000000000");
    claude_cmd.stdin(Stdio::inherit());
    claude_cmd.stdout(Stdio::inherit());
    claude_cmd.stderr(Stdio::inherit());
    let mut claude = claude_cmd.spawn()?;

    let status = claude.wait()?;
    drop(guard);

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn find_free_port() -> Result<u16, Box<dyn std::error::Error>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}


fn launch_aider(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let bin = find_binary(&["aider"])?;
    let mut cmd = Command::new(&bin);
    cmd.arg("--model").arg(format!("openai/{}", model));
    cmd.arg("--openai-api-base").arg(BASE_URL);
    cmd.args(extra_args);
    cmd.env("OPENAI_API_KEY", api_key);
    cmd.env("OPENAI_API_BASE", BASE_URL);
    Err(cmd.exec().into())
}

fn launch_goose(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let bin = find_binary(&["goose", "goose-tui"])?;
    let mut cmd = Command::new(&bin);
    cmd.args(extra_args);
    cmd.env("GOOSE_PROVIDER", "openai");
    cmd.env("GOOSE_MODEL", model);
    cmd.env("OPENAI_API_KEY", api_key);
    cmd.env("OPENAI_HOST", "https://anonkey.st");
    cmd.env("OPENAI_BASE_PATH", "v1");
    Err(cmd.exec().into())
}

fn launch_opencode(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let bin = find_binary(&["opencode"])?;
    let config = serde_json::json!({
        "provider": {
            "openai": {
                "apiKey": api_key,
                "baseURL": BASE_URL
            }
        },
        "agents": {
            "default": {
                "model": format!("openai/{}", model)
            }
        }
    });
    let mut cmd = Command::new(&bin);
    cmd.args(extra_args);
    cmd.env("OPENCODE_CONFIG_CONTENT", config.to_string());
    cmd.env("OPENAI_API_KEY", api_key);
    cmd.env("OPENAI_BASE_URL", BASE_URL);
    Err(cmd.exec().into())
}

fn launch_copilot(api_key: &str, model: &str, extra_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let bin = find_binary(&["github-copilot", "copilot"])?;
    let mut cmd = Command::new(&bin);
    cmd.args(extra_args);
    cmd.env("COPILOT_PROVIDER_BASE_URL", BASE_URL);
    cmd.env("COPILOT_PROVIDER_API_KEY", api_key);
    cmd.env("COPILOT_PROVIDER_WIRE_API", "responses");
    cmd.env("COPILOT_MODEL", model);
    Err(cmd.exec().into())
}
